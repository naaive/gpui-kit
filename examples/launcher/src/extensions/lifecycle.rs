//! How long a command's code stays loaded.
//!
//! A command's view, and the policy it was loaded under, stay alive while any
//! of its pages is on the stack and for [`KEEP_ALIVE`] after the last one is
//! popped, so opening the same command again soon is instant and finds it as
//! the user left it. After that the view is dropped, which is the release:
//! GPUI Shell cancels the tasks and timers its application started.
//!
//! This module is only the bookkeeping, with no clock of its own: the host
//! starts a timer when a launch becomes idle and hands back the
//! [`IdleTicket`] it got, and the ticket is honoured only if nothing used the
//! launch in between. That keeps the timing rule a pure function of events.

use std::{collections::BTreeMap, time::Duration};

/// How long an idle command stays loaded.
pub const KEEP_ALIVE: Duration = Duration::from_secs(60);

/// How long a no-view command may keep running without reporting back.
///
/// GPUI Shell does not say whether a view still has pending work, so a no-view
/// command is released when it shows a HUD or closes the window — its way of
/// saying it is done — or after this long, whichever comes first.
pub const BACKGROUND_LIMIT: Duration = Duration::from_secs(30);

/// One opening of one command.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct LaunchId(u64);

/// Permission to release a launch that became idle, if it still is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IdleTicket {
    launch: LaunchId,
    epoch: u64,
}

impl IdleTicket {
    pub fn launch(&self) -> LaunchId {
        self.launch
    }
}

#[derive(Debug, Default)]
struct Usage {
    /// Pages on the stack, or running background work, that use the launch.
    users: usize,
    /// Advances whenever the launch becomes idle, so a ticket from an earlier
    /// idle period is recognized as stale.
    epoch: u64,
}

#[derive(Debug, Default)]
pub struct Lifecycle {
    launches: BTreeMap<LaunchId, Usage>,
    next: u64,
}

impl Lifecycle {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a new launch, not yet used by anything.
    pub fn start(&mut self) -> LaunchId {
        self.next += 1;
        let id = LaunchId(self.next);
        self.launches.insert(id, Usage::default());
        id
    }

    /// A page of `launch` was pushed, or its background work started.
    pub fn acquire(&mut self, launch: LaunchId) {
        if let Some(usage) = self.launches.get_mut(&launch) {
            usage.users += 1;
        }
    }

    /// A page of `launch` left the stack, or its background work ended.
    ///
    /// Returns a ticket when that was the last user; releasing the launch
    /// after [`KEEP_ALIVE`] is then up to the caller.
    pub fn release_user(&mut self, launch: LaunchId) -> Option<IdleTicket> {
        let usage = self.launches.get_mut(&launch)?;
        usage.users = usage.users.saturating_sub(1);
        (usage.users == 0).then(|| {
            usage.epoch += 1;
            IdleTicket {
                launch,
                epoch: usage.epoch,
            }
        })
    }

    pub fn is_idle(&self, launch: LaunchId) -> bool {
        self.launches
            .get(&launch)
            .is_some_and(|usage| usage.users == 0)
    }

    pub fn contains(&self, launch: LaunchId) -> bool {
        self.launches.contains_key(&launch)
    }

    /// Forgets the launch the ticket is for and returns `true`, if it has been
    /// idle ever since the ticket was issued.
    pub fn expire(&mut self, ticket: IdleTicket) -> bool {
        let current = self
            .launches
            .get(&ticket.launch)
            .is_some_and(|usage| usage.users == 0 && usage.epoch == ticket.epoch);
        if current {
            self.launches.remove(&ticket.launch);
        }
        current
    }

    /// Forgets a launch now, whatever its state.
    pub fn remove(&mut self, launch: LaunchId) {
        self.launches.remove(&launch);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_launch_is_released_only_after_staying_idle() {
        let mut lifecycle = Lifecycle::new();
        let launch = lifecycle.start();
        lifecycle.acquire(launch);
        lifecycle.acquire(launch);

        assert_eq!(lifecycle.release_user(launch), None, "a page is still open");
        let first = lifecycle
            .release_user(launch)
            .expect("the last page closed");
        assert!(lifecycle.is_idle(launch));

        // Reopened within the keep-alive: the earlier ticket no longer applies.
        lifecycle.acquire(launch);
        assert!(!lifecycle.expire(first));
        assert!(lifecycle.contains(launch));

        let second = lifecycle.release_user(launch).unwrap();
        assert!(!lifecycle.expire(first), "a stale ticket stays stale");
        assert!(lifecycle.expire(second));
        assert!(!lifecycle.contains(launch));
        assert!(!lifecycle.expire(second), "released once");
    }

    #[test]
    fn test_launches_are_independent() {
        let mut lifecycle = Lifecycle::new();
        let (a, b) = (lifecycle.start(), lifecycle.start());
        assert_ne!(a, b);
        lifecycle.acquire(a);
        lifecycle.acquire(b);
        let ticket = lifecycle.release_user(a).unwrap();
        assert_eq!(ticket.launch(), a);
        assert!(!lifecycle.is_idle(b));
        assert!(lifecycle.expire(ticket));
        assert!(lifecycle.contains(b));

        lifecycle.remove(b);
        assert_eq!(lifecycle.release_user(b), None, "a removed launch is gone");
    }
}
