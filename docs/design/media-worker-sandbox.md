# Media worker sandbox wishlist

Media worker sandboxing is a long-term, ultra-low-priority wishlist item after the initial release. CRFty uses operating-system process groups and Windows Jobs to stop its trusted media tools. These manage process lifetime; they do not restrict filesystem access, network access, or system calls.

Linux could use bubblewrap with filesystem, network, and PID namespaces. macOS would need a signed helper and an App Sandbox policy compatible with the supported tool installation paths. Windows could combine AppContainer isolation with the existing Job. Each approach needs an explicit policy for input files, staging outputs, temporary files, tool libraries, and hardware acceleration.

Any future implementation must justify its maintenance cost, work with cancellation and output recovery, and prove its restrictions with native platform tests. Sandboxing does not block the use of the official ab-av1 CLI or ordinary process containment.

References: [bubblewrap](https://github.com/containers/bubblewrap), [macOS App Sandbox](https://developer.apple.com/documentation/security/app-sandbox), and [Windows AppContainer](https://learn.microsoft.com/en-us/windows/win32/secauthz/appcontainer-isolation).
