//! Times a cold and a warm session scan over the real directory.
use std::time::Instant;

use pi_rpc::sessions::{SessionScanner, default_session_dir};

fn main() -> anyhow::Result<()> {
    let dir = default_session_dir().expect("a home directory");
    let mut scanner = SessionScanner::default();

    let cold = Instant::now();
    let groups = scanner.scan(&dir)?;
    let cold = cold.elapsed();
    let files: usize = groups.iter().map(|g| g.sessions.len()).sum();
    println!(
        "cold: {cold:?} for {files} sessions in {} groups",
        groups.len()
    );

    let warm = Instant::now();
    let _ = scanner.scan(&dir)?;
    println!("warm: {:?}", warm.elapsed());
    Ok(())
}
