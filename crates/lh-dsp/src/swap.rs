//! Lock-free asset swapping — the white paper's "garbage chute" (§4.1).
//!
//! Heavy objects (NAM models, IR convolvers) are built on the control thread,
//! installed into a running effect through an SPSC ring, and the replaced
//! object travels back on a second ring to be dropped off the audio thread.
//! The audio side never allocates, deallocates, or blocks.

use rtrb::{Consumer, Producer, PushError, RingBuffer};

/// Audio-thread side: owns the live asset.
pub struct AssetSlot<T: Send> {
    current: Option<Box<T>>,
    incoming: Consumer<Option<Box<T>>>,
    retired: Producer<Box<T>>,
    /// Replaced asset we couldn't retire yet because the chute was full.
    parked: Option<Box<T>>,
}

impl<T: Send> AssetSlot<T> {
    /// Accept a pending install/clear, sending the old asset down the chute.
    /// RT-safe; call once per process block.
    pub fn tick(&mut self) {
        if let Some(old) = self.parked.take()
            && let Err(PushError::Full(old)) = self.retired.push(old)
        {
            self.parked = Some(old);
            return; // chute still full — hold new work until it drains
        }
        if let Ok(next) = self.incoming.pop()
            && let Some(old) = std::mem::replace(&mut self.current, next)
            && let Err(PushError::Full(old)) = self.retired.push(old)
        {
            self.parked = Some(old);
        }
    }

    pub fn get_mut(&mut self) -> Option<&mut T> {
        self.current.as_deref_mut()
    }

    pub fn is_loaded(&self) -> bool {
        self.current.is_some()
    }
}

/// Control-thread side: installs assets and drops retired ones.
pub struct AssetHandle<T: Send> {
    tx: Producer<Option<Box<T>>>,
    retired_rx: Consumer<Box<T>>,
}

/// Returned by [`AssetHandle::install`] when the ring is full; carries the
/// asset back so the caller can retry once the audio thread catches up.
pub struct InstallFull<T>(pub Box<T>);

impl<T> std::fmt::Debug for InstallFull<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("install ring full")
    }
}

impl<T: Send> AssetHandle<T> {
    /// Queue a new asset for installation. Returns the asset back if the
    /// install ring is full (retry after the audio thread catches up).
    /// Callers own garbage collection — call [`Self::collect_garbage`]
    /// periodically (e.g. every control-loop tick).
    pub fn install(&mut self, asset: Box<T>) -> Result<(), InstallFull<T>> {
        self.tx
            .push(Some(asset))
            .map_err(|PushError::Full(v)| InstallFull(v.expect("install ring item is always Some")))
    }

    /// Queue removal of the current asset. Returns false if the ring is full.
    pub fn clear(&mut self) -> bool {
        self.tx.push(None).is_ok()
    }

    /// Drop retired assets here, off the audio thread. Returns how many died.
    pub fn collect_garbage(&mut self) -> usize {
        let mut n = 0;
        while self.retired_rx.pop().is_ok() {
            n += 1;
        }
        n
    }
}

/// Wire an effect-side slot to a control-side handle.
pub fn asset_channel<T: Send>() -> (AssetSlot<T>, AssetHandle<T>) {
    let (tx, incoming) = RingBuffer::new(4);
    let (retired, retired_rx) = RingBuffer::new(8);
    (
        AssetSlot {
            current: None,
            incoming,
            retired,
            parked: None,
        },
        AssetHandle { tx, retired_rx },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_swap_and_retire_lifecycle() {
        let (mut slot, mut handle) = asset_channel::<u32>();
        assert!(!slot.is_loaded());

        handle.install(Box::new(1)).unwrap();
        slot.tick();
        assert_eq!(slot.get_mut(), Some(&mut 1));

        handle.install(Box::new(2)).unwrap();
        slot.tick();
        assert_eq!(slot.get_mut(), Some(&mut 2));
        assert_eq!(handle.collect_garbage(), 1, "old asset came back to die");

        assert!(handle.clear());
        slot.tick();
        assert!(!slot.is_loaded());
        assert_eq!(handle.collect_garbage(), 1);
    }

    #[test]
    fn full_chute_parks_the_old_asset_without_losing_it() {
        let (mut slot, mut handle) = asset_channel::<u32>();
        // Swap more times than the retire ring (8) can hold, never collecting.
        let mut installed = 0;
        for i in 0..12 {
            if handle.install(Box::new(i)).is_ok() {
                installed += 1;
            }
            slot.tick();
        }
        // Everything installed and replaced must eventually come back.
        let mut collected = handle.collect_garbage();
        for _ in 0..4 {
            slot.tick(); // drain parked
            collected += handle.collect_garbage();
        }
        assert_eq!(collected, installed - 1, "all replaced assets retired");
        assert!(slot.is_loaded());
    }
}
