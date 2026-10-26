//! Run with: cargo run -p daw-core --example session_bench --release
use daw_core::{
    project::Project,
    session::{Command, Edit, Request, Session},
};
fn main() {
    let mut session = Session::new(Project::default_project()).unwrap();
    let mut timings = Vec::new();
    for index in 0..1000 {
        let request = Request {
            version: 1,
            operation_id: index.to_string(),
            expected_revision: Some(session.revision),
            command: Command::Batch {
                label: "benchmark".into(),
                edits: vec![Edit::Tempo {
                    bpm: 100. + (index % 50) as f64,
                }],
            },
        };
        let start = std::time::Instant::now();
        assert!(session.execute(&request).error.is_none());
        timings.push(start.elapsed().as_micros());
    }
    timings.sort_unstable();
    println!(
        "session transactions: n=1000 median={}us p95={}us max={}us",
        timings[500], timings[950], timings[999]
    );
}
