//! Vendor task execution: the worker body behind an Install or Check
//! request, and the tool-discovery refresh both settle into.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use crfty_core::{Command, Reply, SystemCommand, VendorActivity};

use crate::{
    driver::CommandSender,
    vendor::{
        discovery::{self, CurrentTools, DiscoveredTools, DiscoveryReport},
        download::HttpFetch,
        install::{self as vendor_install, InstallError, InstallProgress},
        manifest,
    },
};

use super::{EngineConfig, ToolsConfig};

#[derive(Clone, Copy)]
pub(super) enum VendorTask {
    Install,
    Check,
}

/// Download progress is re-reported only every this many bytes so the
/// ordered stream carries a handful of updates per archive, not one per
/// 128 KiB chunk.
const VENDOR_PROGRESS_STEP_BYTES: u64 = 8 * 1024 * 1024;

pub(super) fn spawn_vendor_worker(
    commands: &CommandSender,
    config: &EngineConfig,
    tools_slot: &Arc<Mutex<Option<CurrentTools>>>,
    cancelled: &Arc<AtomicBool>,
    task: VendorTask,
) -> Option<thread::JoinHandle<()>> {
    cancelled.store(false, Ordering::Relaxed);
    let worker_commands = commands.clone();
    let worker_config = config.clone();
    let worker_tools = Arc::clone(tools_slot);
    let worker_cancelled = Arc::clone(cancelled);
    let spawned = thread::Builder::new()
        .name("crfty-vendor-worker".to_owned())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_vendor_task(
                    &worker_commands,
                    &worker_config,
                    &worker_tools,
                    &worker_cancelled,
                    task,
                );
            }));
            if result.is_err() {
                submit_vendor_activity(
                    &worker_commands,
                    VendorActivity::Failed {
                        detail: "the vendor worker panicked".to_owned(),
                    },
                );
            }
        });
    match spawned {
        Ok(handle) => Some(handle),
        Err(error) => {
            submit_vendor_activity(
                commands,
                VendorActivity::Failed {
                    detail: format!("failed to start the vendor worker: {error}"),
                },
            );
            None
        }
    }
}

fn run_vendor_task(
    commands: &CommandSender,
    config: &EngineConfig,
    tools_slot: &Mutex<Option<CurrentTools>>,
    cancelled: &AtomicBool,
    task: VendorTask,
) {
    match task {
        VendorTask::Install => match run_vendor_install(commands, config, cancelled) {
            Ok(()) => {
                refresh_discovered_tools(commands, config, tools_slot);
                submit_vendor_activity(commands, VendorActivity::Idle);
            }
            // A cancelled install is not a failure: the previous tools are
            // untouched, so the activity simply returns to rest.
            Err(InstallError::Cancelled) => submit_vendor_activity(commands, VendorActivity::Idle),
            Err(InstallError::Failed(detail)) => {
                submit_vendor_activity(commands, VendorActivity::Failed { detail });
            }
        },
        VendorTask::Check => {
            refresh_discovered_tools(commands, config, tools_slot);
            submit_vendor_activity(commands, VendorActivity::Idle);
        }
    }
}

fn run_vendor_install(
    commands: &CommandSender,
    config: &EngineConfig,
    cancelled: &AtomicBool,
) -> Result<(), InstallError> {
    if matches!(config.tools, ToolsConfig::Fixed(_)) {
        return Err(InstallError::Failed(
            "this engine instance runs with a fixed tool set".to_owned(),
        ));
    }
    let Some(manifest) = manifest::current() else {
        return Err(InstallError::Failed(
            "no pinned FFmpeg build exists for this platform".to_owned(),
        ));
    };
    let fetch = HttpFetch::new().map_err(InstallError::Failed)?;
    let mut last_reported: u64 = 0;
    let mut progress = |event: InstallProgress| match event {
        InstallProgress::Downloading { received, total } => {
            let complete = total.is_some_and(|total| received >= total);
            if received.saturating_sub(last_reported) >= VENDOR_PROGRESS_STEP_BYTES || complete {
                last_reported = received;
                submit_vendor_activity(commands, VendorActivity::Downloading { received, total });
            }
        }
        InstallProgress::Installing => {
            submit_vendor_activity(commands, VendorActivity::Installing);
        }
    };
    vendor_install::install(
        &config.vendor_root,
        manifest,
        &fetch,
        &mut progress,
        cancelled,
    )
    .map(|_metadata| ())
}

/// Re-runs discovery, publishes the result, and updates the shared slot the
/// next session snapshots. Discovery reads the freshly written
/// `current.json`, so a successful install becomes active tools without
/// trusting anything but the on-disk record — and explicit env overrides
/// still win.
fn refresh_discovered_tools(
    commands: &CommandSender,
    config: &EngineConfig,
    tools_slot: &Mutex<Option<CurrentTools>>,
) {
    let report = match &config.tools {
        ToolsConfig::Discover => discovery::discover(&config.vendor_root),
        ToolsConfig::Fixed(tools) => DiscoveryReport {
            tools: tools.clone(),
            update_available: false,
        },
    };
    let current = match &report.tools {
        DiscoveredTools::Available(current) => Some(current.clone()),
        DiscoveredTools::Missing { .. } => None,
    };
    {
        let mut slot = match tools_slot.lock() {
            Ok(slot) => slot,
            Err(poisoned) => poisoned.into_inner(),
        };
        *slot = current;
    }
    match commands.submit(Command::System(SystemCommand::ToolsDiscovered {
        availability: report.tools.availability(),
        update_available: report.update_available,
    })) {
        Ok(Reply::Accepted) => {}
        Ok(reply) => tracing::warn!("tool rediscovery report was not accepted: {reply:?}"),
        Err(error) => tracing::warn!("failed to report rediscovered tools: {error}"),
    }
}

fn submit_vendor_activity(commands: &CommandSender, activity: VendorActivity) {
    match commands.submit(Command::System(SystemCommand::VendorProgress { activity })) {
        Ok(Reply::Accepted) => {}
        Ok(reply) => tracing::warn!("vendor progress report was not accepted: {reply:?}"),
        Err(error) => tracing::warn!("failed to report vendor progress: {error}"),
    }
}
