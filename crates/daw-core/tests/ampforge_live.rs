//! Real AmpForge compatibility checks. Install the upstream Linux CLAP first;
//! run explicitly with `cargo test -p daw-core --test ampforge_live -- --ignored`.
use daw_core::{
    audio::{
        clap::ClapInstance,
        input::{run_writer, CaptureShared, InputChannels, WriterCmd},
        live_plugin::{LivePlugin, AMPFORGE_ID},
    },
    project::ClapSlot,
};
use ringbuf::{
    traits::{Observer, Producer, Split},
    HeapRb,
};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

fn slot() -> ClapSlot {
    let path = std::env::var_os("DAW_AMPFORGE_TEST_PLUGIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap())
                .join(".clap/AmpForge/ampforge_main.clap")
        });
    assert!(
        path.is_file(),
        "install AmpForge or set DAW_AMPFORGE_TEST_PLUGIN"
    );
    let instance = ClapInstance::load(path.to_str().unwrap(), AMPFORGE_ID, 48_000, 8192).unwrap();
    let mut params = std::collections::BTreeMap::new();
    for (name, value) in [("Amp Drive", 18.), ("Amp Volume", -12.), ("Gate On", 0.)] {
        let info = instance
            .handle
            .param_infos
            .iter()
            .find(|p| p.name == name)
            .expect(name);
        params.insert(info.id.to_string(), value);
    }
    instance.shutdown();
    ClapSlot {
        uid: "guitar-ampforge".into(),
        path: path.to_string_lossy().into_owned(),
        id: AMPFORGE_ID.into(),
        state: None,
        params,
    }
}

#[test]
#[ignore = "requires the external AmpForge CLAP"]
fn ampforge_mono_audio_and_saved_state_round_trip() {
    let slot = slot();
    let mut instance = ClapInstance::load(&slot.path, &slot.id, 48_000, 256).unwrap();
    let input: Vec<f32> = (0..256).map(|i| (i as f32 * 0.08).sin() * 0.06).collect();
    let mut left = input.clone();
    let mut right = input.clone();
    instance
        .processor
        .as_mut()
        .unwrap()
        .process(&mut left, &mut right);
    assert_eq!(
        left, right,
        "mono plugin output is duplicated, not lost on the right"
    );
    assert!(left.iter().any(|x| x.abs() > 0.001));
    assert!(!instance.processor.as_ref().unwrap().process_failed);
    instance.shutdown();

    let (mut owner, mut processor) = LivePlugin::load(&slot, 44_100).unwrap();
    let saved = owner.snapshot();
    assert!(saved.state.as_ref().is_some_and(|s| !s.is_empty()));
    for (id, value) in &slot.params {
        assert!((saved.params[id] - value).abs() < 1e-6);
    }
    let mut audio = input.clone();
    processor.process(&mut audio);
    assert!(audio.iter().all(|x| x.is_finite() && x.abs() <= 1.));
    drop(processor);
    drop(owner);
    let (mut restored, processor) = LivePlugin::load(&saved, 48_000).unwrap();
    let snapshot = restored.snapshot();
    for (id, value) in &slot.params {
        assert!((snapshot.params[id] - value).abs() < 1e-6);
    }
    drop(processor);
    drop(restored);
}

#[test]
#[ignore = "requires the external AmpForge CLAP"]
fn recording_writer_prints_ampforge_instead_of_dry_guitar() {
    let slot = slot();
    let input: Vec<f32> = (0..4096).map(|i| (i as f32 * 0.1).sin() * 0.06).collect();
    let (expected_owner, mut expected_processor) = LivePlugin::load(&slot, 44_100).unwrap();
    let mut expected = input.clone();
    expected_processor.process(&mut expected);
    drop(expected_processor);
    drop(expected_owner);
    let (owner, processor) = LivePlugin::load(&slot, 44_100).unwrap();
    let (mut producer, consumer) = HeapRb::<f32>::new(8192).split();
    assert_eq!(producer.push_slice(&input), input.len());
    let gate = Arc::new(AtomicU32::new(1));
    let worker_gate = gate.clone();
    let (tx, rx) = crossbeam_channel::unbounded();
    let (takes_tx, takes_rx) = crossbeam_channel::unbounded();
    let directory =
        std::env::temp_dir().join(format!("daw-ampforge-writer-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    tx.send(WriterCmd::Plugin(Box::new(processor))).unwrap();
    tx.send(WriterCmd::StartSession {
        base: directory.join("take"),
    })
    .unwrap();
    let writer = std::thread::spawn(move || {
        run_writer(
            consumer,
            Arc::new(CaptureShared::new()),
            1,
            InputChannels::Mono(0),
            44_100,
            worker_gate,
            Arc::new(AtomicU32::new(480)),
            rx,
            takes_tx,
        )
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while producer.occupied_len() > 0 {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    gate.store(2, Ordering::Release);
    let take = takes_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    tx.send(WriterCmd::Shutdown).unwrap();
    writer.join().unwrap();
    drop(owner);
    let recorded: Vec<f32> = hound::WavReader::open(&take.path)
        .unwrap()
        .samples::<f32>()
        .map(Result::unwrap)
        .collect();
    assert_eq!(recorded.len(), input.len());
    assert!(
        recorded
            .iter()
            .zip(&input)
            .any(|(a, b)| (a - b).abs() > 0.01),
        "take must be processed"
    );
    assert!(
        recorded
            .iter()
            .zip(expected)
            .all(|(a, b)| (a - b).abs() < 1e-6),
        "recorded tone uses the same AmpForge processor"
    );
    assert_eq!(take.start_tick, 480);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
#[ignore = "requires AmpForge and an X11/XWayland display"]
fn ampforge_native_editor_opens_pumps_and_closes() {
    let (mut owner, mut processor) = LivePlugin::load(&slot(), 48_000).unwrap();
    owner.show_editor().unwrap();
    for _ in 0..30 {
        processor.process(&mut [0.; 128]);
        owner.pump();
        std::thread::sleep(Duration::from_millis(16));
    }
    owner.hide_editor();
    assert!(owner.snapshot().state.is_some());
    drop(processor);
    drop(owner);
}
