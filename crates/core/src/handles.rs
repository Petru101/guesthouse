//! Handles the host gives the guest.

const FIRST_HANDLE: u32 = 0x4000_1040;
const STEP: u32 = 0x40;

/// One sequence for every kind of object, so a handle never means two things at once.
#[derive(Debug)]
pub struct HandleCounter {
    next: u32,
}

impl HandleCounter {
    pub fn new() -> Self {
        Self { next: FIRST_HANDLE }
    }

    pub fn take(&mut self) -> u32 {
        let handle = self.next;
        // Tens of millions of objects later this comes to the end of the numbers. Starting again
        // costs nothing, as whatever still has an old number has outlived that many others.
        self.next = self.next.checked_add(STEP).unwrap_or(FIRST_HANDLE);
        handle
    }
}

impl Default for HandleCounter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_are_spaced_evenly() {
        let mut handles = HandleCounter::new();
        assert_eq!(handles.take(), FIRST_HANDLE);
        assert_eq!(handles.take(), FIRST_HANDLE + STEP);
    }

    #[test]
    fn handles_start_again_after_the_last() {
        let mut handles = HandleCounter { next: u32::MAX - STEP + 1 };
        assert_eq!(handles.take(), u32::MAX - STEP + 1);
        assert_eq!(handles.take(), FIRST_HANDLE);
    }
}
