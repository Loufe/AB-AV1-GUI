use super::*;
use process_wrap::tokio::CommandWrapper;
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Stdio,
    thread,
    time::{Duration, Instant},
};

const FIXTURE_DIRECTORY: &str = "CRFTY_WINDOWS_SPAWN_DIRECTORY";
const TREE_FIXTURE: &str = "process::windows::tests::tree_fixture";
const LEAF_FIXTURE: &str = "process::windows::tests::leaf_fixture";
const READY_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug)]
struct FailSetup {
    ready: Option<PathBuf>,
}

impl CommandWrapper for FailSetup {
    fn wrap_child(
        &mut self,
        _child: Box<dyn ChildWrapper>,
        _core: &CommandWrap,
    ) -> io::Result<Box<dyn ChildWrapper>> {
        if let Some(ready) = &self.ready {
            wait_for_file(ready)?;
        }
        Err(io::Error::from_raw_os_error(5))
    }
}

#[test]
fn failed_setup_reaps_its_suspended_native_child() -> Result<(), Box<dyn std::error::Error>> {
    let mut command = TokioCommand::new(env::current_exe()?);
    command
        .arg("--list")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut wrapped = CommandWrap::from(command);
    wrapped
        .wrap(KillOnDrop)
        .wrap(CreationFlags(
            ::windows::Win32::System::Threading::CREATE_NO_WINDOW,
        ))
        .wrap(FailSetup { ready: None })
        .wrap(JobObject);
    let cleanup = CleanupNotes(Arc::new(Mutex::new(Some(Vec::new()))));
    let mut pid = None;
    let error = wrapped
        .spawn_with_child(|command| {
            let child = command.as_std_mut().spawn()?;
            pid = Some(child.id());
            Ok(Box::new(NativeChild::new(child, cleanup.clone())))
        })
        .err()
        .ok_or("setup unexpectedly succeeded")?;
    assert_eq!(error.raw_os_error(), Some(5));
    assert!(cleanup.finish().is_empty());
    assert_process_exited(pid.ok_or("native child did not spawn")?)?;
    Ok(())
}

#[test]
fn failed_setup_after_job_assignment_terminates_a_live_tree()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let ready = directory.path().join("ready");
    let mut command = TokioCommand::from(helper(TREE_FIXTURE)?);
    command.env(FIXTURE_DIRECTORY, directory.path());
    let mut wrapped = CommandWrap::from(command);
    wrapped
        .wrap(KillOnDrop)
        .wrap(CreationFlags(
            ::windows::Win32::System::Threading::CREATE_NO_WINDOW,
        ))
        .wrap(JobObject)
        .wrap(FailSetup {
            ready: Some(ready.clone()),
        });
    let cleanup = CleanupNotes(Arc::new(Mutex::new(Some(Vec::new()))));
    let mut pid = None;
    let error = wrapped
        .spawn_with_child(|command| {
            let child = command.as_std_mut().spawn()?;
            pid = Some(child.id());
            Ok(Box::new(NativeChild::new(child, cleanup.clone())))
        })
        .err()
        .ok_or("setup unexpectedly succeeded")?;
    assert_eq!(error.raw_os_error(), Some(5));
    assert!(cleanup.finish().is_empty());
    assert_process_exited(pid.ok_or("native child did not spawn")?)?;
    let descendant = fs::read_to_string(ready)?.parse()?;
    assert_process_exited(descendant)?;
    let heartbeat = directory.path().join("heartbeat");
    let stopped = fs::read(&heartbeat)?;
    thread::sleep(Duration::from_millis(200));
    assert_eq!(fs::read(heartbeat)?, stopped);
    Ok(())
}

#[test]
fn cleanup_failure_preserves_the_initiating_error_and_every_step() {
    let failure = SpawnFailure {
        primary: io::Error::from_raw_os_error(5),
        notes: vec!["terminate child failed".into(), "reap child failed".into()],
    };
    let report = failure.to_string();
    assert!(report.contains("terminate child failed"));
    assert!(report.contains("reap child failed"));
    assert_eq!(failure.primary.raw_os_error(), Some(5));
    assert!(std::error::Error::source(&failure).is_some());
}

fn helper(name: &str) -> io::Result<Command> {
    let mut command = Command::new(env::current_exe()?);
    command
        .args(["--exact", name, "--ignored", "--nocapture"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    Ok(command)
}

fn wait_for_file(path: &Path) -> io::Result<()> {
    let deadline = Instant::now() + READY_TIMEOUT;
    while !path.exists() {
        if Instant::now() >= deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "fixture not ready"));
        }
        thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

fn assert_process_exited(pid: u32) -> io::Result<()> {
    let tasklist = PathBuf::from(
        env::var_os("SystemRoot")
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "SystemRoot missing"))?,
    )
    .join("System32/tasklist.exe");
    let output = Command::new(tasklist)
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()?;
    assert!(output.status.success());
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\"")),
        "process {pid} survived failed setup"
    );
    Ok(())
}

#[test]
#[ignore = "subprocess helper"]
fn tree_fixture() -> Result<(), Box<dyn std::error::Error>> {
    let directory =
        PathBuf::from(env::var_os(FIXTURE_DIRECTORY).ok_or("fixture directory missing")?);
    let mut child = helper(LEAF_FIXTURE)?.spawn()?;
    wait_for_file(&directory.join("heartbeat"))?;
    fs::write(directory.join("ready-staging"), child.id().to_string())?;
    fs::rename(directory.join("ready-staging"), directory.join("ready"))?;
    child.wait()?;
    Ok(())
}

#[test]
#[ignore = "subprocess helper"]
fn leaf_fixture() -> Result<(), Box<dyn std::error::Error>> {
    let directory =
        PathBuf::from(env::var_os(FIXTURE_DIRECTORY).ok_or("fixture directory missing")?);
    let mut counter = 0_u64;
    loop {
        fs::write(directory.join("heartbeat"), counter.to_string())?;
        counter = counter.wrapping_add(1);
        thread::sleep(Duration::from_millis(20));
    }
}
