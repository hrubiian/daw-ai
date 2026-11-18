//! ALSA worker shutdown, kept independent of a PCM device for regression tests.
use std::io;
use std::sync::Arc;
use std::thread::JoinHandle;

pub(super) struct TriggerSender(libc::c_int);
pub(super) struct TriggerReceiver(pub(super) libc::c_int);

fn transfer(mut operation: impl FnMut() -> io::Result<usize>) -> io::Result<()> {
    loop {
        match operation() {
            Ok(8) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
            Ok(count) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    format!("ALSA shutdown pipe transferred {count} bytes instead of 8"),
                ))
            }
        }
    }
}

impl TriggerSender {
    fn wakeup(&self) -> io::Result<()> {
        let value = 1u64;
        transfer(|| {
            let count = unsafe { libc::write(self.0, &value as *const u64 as *const _, 8) };
            if count < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(count as usize)
            }
        })
    }
}

impl TriggerReceiver {
    pub(super) fn clear_pipe(&self) -> io::Result<()> {
        let mut value = 0u64;
        transfer(|| {
            let count = unsafe { libc::read(self.0, &mut value as *mut u64 as *mut _, 8) };
            if count < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(count as usize)
            }
        })
    }
}

pub(super) fn trigger() -> (TriggerSender, Arc<TriggerReceiver>) {
    let mut fds = [0, 0];
    match unsafe { libc::pipe(fds.as_mut_ptr()) } {
        0 => (TriggerSender(fds[1]), Arc::new(TriggerReceiver(fds[0]))),
        _ => panic!(
            "Could not create ALSA shutdown pipe: {}",
            io::Error::last_os_error()
        ),
    }
}

impl Drop for TriggerSender {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.0);
        }
    }
}

impl Drop for TriggerReceiver {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.0);
        }
    }
}

pub(super) struct StreamThread {
    thread: Option<JoinHandle<()>>,
    trigger: TriggerSender,
    // A worker can fail before Stream is dropped. Keep its pipe's read end
    // open so waking an already finished worker cannot write to a closed pipe.
    _receiver: Arc<TriggerReceiver>,
}

impl StreamThread {
    pub(super) fn new(
        thread: JoinHandle<()>,
        trigger: TriggerSender,
        receiver: Arc<TriggerReceiver>,
    ) -> Self {
        Self {
            thread: Some(thread),
            trigger,
            _receiver: receiver,
        }
    }
}

impl Drop for StreamThread {
    fn drop(&mut self) {
        let _ = self.trigger.wakeup();
        if let Some(thread) = self.thread.take() {
            // A failed worker has already reported its original error. Joining
            // it must not cause a second panic, especially during unwinding.
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn retries_interrupted_pipe_io() {
        let mut calls = 0;
        transfer(|| {
            calls += 1;
            if calls < 3 {
                Err(io::ErrorKind::Interrupted.into())
            } else {
                Ok(8)
            }
        })
        .unwrap();
        assert_eq!(calls, 3);
    }

    #[test]
    fn permanent_errors_and_short_transfers_return_without_retrying() {
        let mut calls = 0;
        let error = transfer(|| {
            calls += 1;
            Err(io::ErrorKind::BrokenPipe.into())
        })
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        assert_eq!(calls, 1);
        assert_eq!(
            transfer(|| Ok(0)).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn closing_a_live_worker_wakes_and_joins_it() {
        let (tx, rx) = trigger();
        let worker_rx = Arc::clone(&rx);
        let worker = std::thread::spawn(move || worker_rx.clear_pipe().unwrap());
        drop(StreamThread::new(worker, tx, rx));
    }

    #[test]
    fn closing_after_an_early_exit_or_panic_keeps_the_pipe_alive() {
        for panics in [false, true] {
            let (tx, rx) = trigger();
            let worker_rx = Arc::clone(&rx);
            let (exited_tx, exited_rx) = mpsc::channel();
            let worker = std::thread::spawn(move || {
                drop(worker_rx);
                exited_tx.send(()).unwrap();
                if panics {
                    panic!("simulated audio callback failure");
                }
            });
            exited_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            let lifecycle = StreamThread::new(worker, tx, Arc::clone(&rx));
            // This is the failing syscall from CPAL 0.15.3, after the worker
            // dropped its receiver. Assert success before exercising Drop.
            lifecycle.trigger.wakeup().unwrap();
            rx.clear_pipe().unwrap();
            drop(lifecycle);
        }
    }
}
