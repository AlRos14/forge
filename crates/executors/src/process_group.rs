#[cfg(unix)]
use command_group::AsyncCommandGroup;
use command_group::AsyncGroupChild;
use std::io;
use std::process::ExitStatus;
use tokio::process::{Child, Command};

/// Owns one harness Execution's process group.
///
/// On Unix this stores the dedicated PGID separately because command-group's
/// cached leader status does not report whether orphaned group descendants are
/// still present. On other platforms command-group supplies its Job Object
/// boundary, but its public API cannot verify that the whole job has exited;
/// termination therefore fails closed there.
pub struct ProcessGroupChild {
    child: AsyncGroupChild,
    #[cfg(unix)]
    pgid: i32,
    retired: bool,
}

impl ProcessGroupChild {
    pub fn spawn(command: &mut Command) -> io::Result<Self> {
        #[cfg(unix)]
        {
            // Keep Tokio's direct-child fallback as well as the group boundary.
            command.kill_on_drop(true);
            let child = command.group_spawn()?;
            let pgid = child
                .id()
                .and_then(|id| i32::try_from(id).ok())
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::Other,
                        "process group has no usable leader ID",
                    )
                })?;

            Ok(Self {
                child,
                pgid,
                retired: false,
            })
        }

        #[cfg(not(unix))]
        {
            let _ = command;
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "remote harness execution requires verified Unix process-group retirement",
            ))
        }
    }

    pub fn inner(&mut self) -> &mut Child {
        self.child.inner()
    }

    /// Returns the leader PID while it is still available from Tokio.
    pub fn id(&self) -> Option<u32> {
        self.child.id()
    }

    /// Polls the leader status. This does not prove the process group is empty.
    pub fn try_wait_leader(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    /// Waits for the leader status. Use `kill_and_wait` to verify the complete
    /// process boundary before considering an Execution retired.
    pub async fn wait_leader(&mut self) -> io::Result<ExitStatus> {
        self.child.wait().await
    }

    /// Sends the process-group termination signal without waiting.
    pub fn start_kill(&mut self) -> io::Result<()> {
        self.child.start_kill()
    }

    #[cfg(unix)]
    pub fn send_sigterm(&self) -> io::Result<()> {
        use nix::{
            errno::Errno,
            sys::signal::{killpg, Signal},
            unistd::Pid,
        };

        match killpg(Pid::from_raw(self.pgid), Some(Signal::SIGTERM)) {
            Ok(()) | Err(Errno::ESRCH) => Ok(()),
            Err(error) => Err(io::Error::from(error)),
        }
    }

    /// Whether the execution-owned process group still exists.
    #[cfg(unix)]
    pub fn group_is_alive(&self) -> io::Result<bool> {
        use nix::{errno::Errno, sys::signal::killpg, unistd::Pid};

        match killpg(Pid::from_raw(self.pgid), None) {
            Ok(()) => Ok(true),
            Err(Errno::ESRCH) => Ok(false),
            Err(error) => Err(io::Error::from(error)),
        }
    }

    /// Kill the execution-owned process group, reap the leader, and wait until
    /// no process remains in the group. A cached leader exit is not evidence
    /// that descendants have exited.
    pub async fn kill_and_wait(&mut self) -> io::Result<ExitStatus> {
        if self.retired {
            return self.child.wait().await;
        }

        #[cfg(unix)]
        {
            use nix::libc;

            let group_alive = self.group_is_alive()?;
            if group_alive {
                if let Err(error) = self.child.start_kill() {
                    if error.raw_os_error() != Some(libc::ESRCH) {
                        return Err(error);
                    }
                }
            }

            let status = self.child.wait().await?;
            while self.group_is_alive()? {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            self.retired = true;
            Ok(status)
        }

        #[cfg(not(unix))]
        {
            self.child.start_kill()?;
            let _ = self.child.wait().await?;
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "the command-group Job Object API cannot verify that every job member exited",
            ))
        }
    }
}

impl Drop for ProcessGroupChild {
    fn drop(&mut self) {
        if !self.retired {
            // A last-resort signal only. Successful retirement still requires
            // the asynchronous `kill_and_wait` verification above.
            let _ = self.child.start_kill();
        }
    }
}
