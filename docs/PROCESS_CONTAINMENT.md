# Process containment

CRFty's direct processes and the ab-av1 adapter use the maintained [`process-wrap`](https://github.com/watchexec/process-wrap) release. Direct Unix processes use its standard-library frontend; Windows processes and ab-av1 use its Tokio frontend. The engine retains responsibility for cancellation, bounded output capture, concurrent pipe drainage, reader joins, and terminal reports. [ADR-018](adr/018-unify-job-cancellation-and-completion.md) owns the broader job supervision design.

## Platform boundary

On Unix, each contained leader starts a fresh process group. Termination signals the group and reaps the direct child. This does not let the application reap arbitrary grandchildren or contain a descendant that deliberately leaves the group.

On Windows, process creation retains `CREATE_NO_WINDOW`. The dependency creates the child suspended, assigns it to an owned Job Object, and resumes it only after assignment. Its Tokio `KillOnDrop` marker enables the Job's kill-on-close limit, so Windows terminates its members if the application exits without running Rust destructors. Direct processes use the wrapper's custom-spawner API to retain a standard-library child and blocking pipes. A current-thread runtime owned on the blocking supervisor or remux thread drives the wrapper's asynchronous Job wait; it outlives the child.

The containment owner observes the leader through the inner child. Leader exit alone does not settle its group or Job. Cleanup requests containment termination, waits for the direct child, and completes the dependency's wait before publishing a report. The Windows completion-port wait is not an independent proof that every descendant has exited: the published dependency accepts any completion notification, and Job closure requests termination asynchronously. Native process and heartbeat tests verify the observable cleanup paths. The ab-av1 child mutex is released between leader polls so a sample producer awaiting exit cannot block cancellation from acquiring termination authority.

## Failed Windows spawn

The maintained dependency owns native Job, completion-port, snapshot, and thread handles during setup. Direct processes additionally own the standard-library child before any wrapper setup can fail. That owner terminates and reaps an unfinished child when setup fails, including a child still suspended before Job assignment. Once assigned, the Job's kill-on-close policy also requests termination of descendants when wrapper setup fails.

The native child owner records cleanup failures outside its destructor so a failed spawn retains the initiating error and each observed child-termination or reap failure. A failed termination request does not trigger an unbounded wait for a child that may remain suspended. Errors suppressed inside the dependency's native Job operations are not exposed by its published API.

## Dependency ownership and coverage

The engine consumes the published `process-wrap` crate directly, with no patch or fork dependency. The reviewed ab-av1 library revision uses the official upstream URL; [ADR-003](adr/003-embed-a-pinned-ab-av1-adapter.md) owns its typed library boundary and upstream contribution strategy.

Windows tests inject wrapper setup failure before Job assignment, while the native child is suspended, and after assignment and resumption, while a real descendant produces a heartbeat. They verify native process exit and preserve the initiating error. The `process_supervisor` contracts cover nested Windows Jobs and application exit with a live descendant, alongside cancellation, timeout, inherited pipes, and large concurrent output. The adapter's [probe contracts](AB_AV1_PROBES.md) exercise native containment through search and encode work. Native Windows execution is necessary to validate Windows behaviour; compiling that target establishes only type correctness.
