//! Single active audio source arbitration (`Source`, `Arbiter`, `Lease`).
//! Ownership: audio-dlna (T1).
//!
//! One virtual receiver plays exactly one primary source at a time. A new
//! source *takes over*: the previous source is suspended (its protocol session
//! is kept alive), queued PCM is flushed, and only the new lease's audio is
//! forwarded to the engine. This mirrors the upstream `PlaybackArbiter` idea
//! but is owned by the Windows host so MiPlay can join the same arbitration.
//!
//! The arbiter itself never owns audio state; it only issues monotonic leases.
//! `protocols.rs` mirrors `Arbiter::current_source()` into
//! [`crate::audio::AudioEngine::set_active_source`], and the engine drops PCM
//! from every non-active source.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Audio source identifiers shared by the engine and the DLNA player.
///
/// `as_str` is part of the NDJSON contract (`activeMediaSource` /
/// `selectedCoreMediaSource` in `docs/WINDOWS_ARCHITECTURE.md` §5.2), so the
/// strings are stable: `airplay`, `dlna`, `xiaomi_miplay`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    AirPlay,
    Dlna,
    MiPlay,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AirPlay => "airplay",
            Self::Dlna => "dlna",
            Self::MiPlay => "xiaomi_miplay",
        }
    }

    /// Parse a wire/UI string back into a source id.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "airplay" => Some(Self::AirPlay),
            "dlna" => Some(Self::Dlna),
            "xiaomi_miplay" | "miplay" => Some(Self::MiPlay),
            _ => None,
        }
    }
}

impl std::fmt::Display for Source {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl From<fusionplay_core::PcmSource> for Source {
    fn from(source: fusionplay_core::PcmSource) -> Self {
        match source {
            fusionplay_core::PcmSource::AirPlay => Self::AirPlay,
            fusionplay_core::PcmSource::Dlna => Self::Dlna,
            fusionplay_core::PcmSource::MiPlay => Self::MiPlay,
        }
    }
}

/// A granted playback lease. `epoch` increases on every claim, so stale PCM or
/// stale progress events can be discarded by comparing epochs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lease {
    pub source: Source,
    pub epoch: u64,
}

/// Hook invoked (outside the internal lock) when `source` gets suspended by a
/// takeover. Must not call back into the arbiter.
pub type SuspendHook = Arc<dyn Fn(Lease) + Send + Sync>;

#[derive(Default)]
struct Inner {
    current: Option<Lease>,
    epoch: u64,
    last_tag: HashMap<Source, String>,
}

/// Outcome of a [`Arbiter::claim`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Claim {
    pub lease: Lease,
    /// Source that was playing before and is now suspended (if different).
    pub suspended: Option<Lease>,
}

/// Single-active-source arbiter. All methods take a short lock only.
pub struct Arbiter {
    inner: Mutex<Inner>,
    hooks: Mutex<HashMap<Source, SuspendHook>>,
}

impl Default for Arbiter {
    fn default() -> Self {
        Self::new()
    }
}

impl Arbiter {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            hooks: Mutex::new(HashMap::new()),
        }
    }

    pub fn set_suspend_hook(&self, source: Source, hook: SuspendHook) {
        if let Ok(mut hooks) = self.hooks.lock() {
            hooks.insert(source, hook);
        }
    }

    pub fn clear_suspend_hook(&self, source: Source) {
        if let Ok(mut hooks) = self.hooks.lock() {
            hooks.remove(&source);
        }
    }

    pub fn current(&self) -> Option<Lease> {
        self.inner.lock().ok().and_then(|inner| inner.current)
    }

    pub fn current_source(&self) -> Option<Source> {
        self.current().map(|lease| lease.source)
    }

    /// Claim playback for `source`. Returns the new lease and the suspended
    /// previous lease (when a takeover actually happened). Hooks are called
    /// outside the lock; a hook must not call back into the arbiter.
    pub fn claim(&self, source: Source, tag: impl Into<String>) -> Claim {
        let mut suspended = None;
        let mut hook = None;
        let lease;
        {
            let Ok(mut inner) = self.inner.lock() else {
                // A poisoned arbiter (panic while holding the lock) must not
                // take the audio engine down: hand out a fresh lease.
                return Claim {
                    lease: Lease { source, epoch: 0 },
                    suspended: None,
                };
            };
            inner.epoch = inner.epoch.wrapping_add(1).max(1);
            lease = Lease {
                source,
                epoch: inner.epoch,
            };
            if let Some(previous) = inner.current
                && previous.source != source
            {
                suspended = Some(previous);
                hook = self
                    .hooks
                    .lock()
                    .ok()
                    .and_then(|hooks| hooks.get(&previous.source).cloned());
            }
            inner.current = Some(lease);
            inner.last_tag.insert(source, tag.into());
        }
        if let (Some(previous), Some(hook)) = (suspended, hook) {
            hook(previous);
        }
        Claim { lease, suspended }
    }

    /// Release the lease if it is still current (source stopped/session ended).
    pub fn release(&self, lease: Lease) -> bool {
        let Ok(mut inner) = self.inner.lock() else {
            return false;
        };
        if inner.current == Some(lease) {
            inner.current = None;
            true
        } else {
            false
        }
    }

    /// Whether PCM for this lease should be forwarded to the output engine.
    pub fn accepts(&self, lease: Lease) -> bool {
        self.inner
            .lock()
            .map(|inner| inner.current == Some(lease))
            .unwrap_or(false)
    }

    pub fn last_tag(&self, source: Source) -> Option<String> {
        self.inner
            .lock()
            .ok()
            .and_then(|inner| inner.last_tag.get(&source).cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_strings_match_ui_contract() {
        assert_eq!(Source::AirPlay.as_str(), "airplay");
        assert_eq!(Source::Dlna.as_str(), "dlna");
        assert_eq!(Source::MiPlay.as_str(), "xiaomi_miplay");
        assert_eq!(Source::from_wire("xiaomi_miplay"), Some(Source::MiPlay));
        assert_eq!(Source::from_wire("miplay"), Some(Source::MiPlay));
        assert_eq!(Source::from_wire("nope"), None);
        assert_eq!(
            Source::from(fusionplay_core::PcmSource::MiPlay),
            Source::MiPlay
        );
    }

    #[test]
    fn takeover_suspends_previous_source() {
        let arbiter = Arbiter::new();
        let suspended = Arc::new(Mutex::new(Vec::<Lease>::new()));
        let record = Arc::clone(&suspended);
        arbiter.set_suspend_hook(
            Source::AirPlay,
            Arc::new(move |lease| {
                if let Ok(mut guard) = record.lock() {
                    guard.push(lease);
                }
            }),
        );
        let first = arbiter.claim(Source::AirPlay, "airplay_session");
        assert!(arbiter.accepts(first.lease));
        assert!(first.suspended.is_none());

        let second = arbiter.claim(Source::Dlna, "dlna_uri");
        assert!(second.lease.epoch > first.lease.epoch);
        assert_eq!(
            second.suspended.map(|lease| lease.source),
            Some(Source::AirPlay)
        );
        assert_eq!(suspended.lock().unwrap().len(), 1);
        assert!(
            !arbiter.accepts(first.lease),
            "old lease must not reach the engine"
        );
        assert!(arbiter.accepts(second.lease));
        assert_eq!(arbiter.current_source(), Some(Source::Dlna));

        // Same source reclaiming (e.g. next track) does not suspend itself.
        let third = arbiter.claim(Source::Dlna, "dlna_next");
        assert!(third.suspended.is_none());
        assert!(arbiter.accepts(third.lease));
        assert_eq!(arbiter.last_tag(Source::Dlna).as_deref(), Some("dlna_next"));
    }

    #[test]
    fn release_only_affects_current_lease() {
        let arbiter = Arbiter::new();
        let lease = arbiter.claim(Source::MiPlay, "miplay").lease;
        let other = Lease {
            source: Source::AirPlay,
            epoch: 999,
        };
        assert!(!arbiter.release(other));
        assert!(arbiter.release(lease));
        assert_eq!(arbiter.current(), None);
        assert_eq!(arbiter.current_source(), None);
    }

    #[test]
    fn second_claim_gets_a_larger_epoch() {
        let arbiter = Arbiter::new();
        let a = arbiter.claim(Source::Dlna, "a").lease;
        let b = arbiter.claim(Source::Dlna, "b").lease;
        assert!(b.epoch > a.epoch);
    }
}
