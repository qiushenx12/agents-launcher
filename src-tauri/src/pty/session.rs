use portable_pty::{Child, MasterPty};
use std::collections::VecDeque;
use std::io::Write;
use std::sync::{Arc, Mutex as StdMutex};

use crate::tab_cli::TabPermission;

pub struct PtySession {
    pub master: Box<dyn MasterPty + Send>,
    pub writer: Box<dyn Write + Send>,
    pub child: Box<dyn Child + Send + Sync>,
    /// Process ID of the direct child spawned in the PTY; used for cascade cleanup.
    pub child_pid: Option<u32>,
    /// Recent output lines for `tab-read` command (shared with reader thread, capped at 500).
    pub output_lines: Arc<StdMutex<VecDeque<String>>>,
    /// Per-tab permission configuration stored inline to avoid cross-field borrow issues.
    pub permission: TabPermission,
    /// Dynamic title captured from OSC 0 escape sequences (shared with reader thread).
    pub title: Arc<StdMutex<String>>,
    /// Session ID parsed from the `-r` flag in the launch command.
    pub session_id: Option<String>,
}
