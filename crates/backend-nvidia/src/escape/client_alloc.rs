//! A guest's request to make an RM client, answered here.
//!
//! With a client of zero the guest asks again on every call. Nothing is made on the host: controls never use the guest's handles, so a number that isn't zero is all
//! that is needed. A guest's allocation never reaches the RM.
//!
//! Two layouts share this escape. Both start the same, and one has a mark in the next field that
//! moves its answer further along.

use super::{get, put};
use crate::rm::Handle;
use crate::rm::classes::ROOT_CLIENT;

const CLASS: usize = 0x3C;
const LAYOUT_MARK: usize = 0x44;
const MARKED_LAYOUT: u32 = 0x77FE_F81E;
const MARKED_STATUS: usize = 0x4C;
const MARKED_HANDLE: usize = 0x50;
const PLAIN_STATUS: usize = 0x44;
const PLAIN_HANDLE: usize = 0x48;

/// Stands for a client on the guest's side. It belongs to no client on the host.
const GUEST_CLIENT: Handle = 0x6775_0041;

/// Fills in the answer, if this asks for a client. Every other class keeps the usual echo.
pub(super) fn answer(data: &mut [u8]) {
    if get(data, CLASS) != Some(ROOT_CLIENT) {
        return;
    }
    let (status, handle) = if get(data, LAYOUT_MARK) == Some(MARKED_LAYOUT) {
        (MARKED_STATUS, MARKED_HANDLE)
    } else {
        (PLAIN_STATUS, PLAIN_HANDLE)
    };
    put(data, status, crate::rm::STATUS_OK);
    put(data, handle, GUEST_CLIENT);
}

#[cfg(test)]
mod tests {
    use super::super::tests::{device, escape};
    use super::super::{FUNCTION_RM_ALLOC, RESULT, answer as answer_escape};
    use super::*;
    use crate::rm::classes::SUBDEVICE;
    use crate::rm::device::Device;
    use crate::rm::fake::{Call, FakeRm};

    const MARKED_SIZE: usize = 0x54;
    const PLAIN_SIZE: usize = 0x60;

    fn request(class: u32, size: usize) -> Vec<u8> {
        let mut data = escape(FUNCTION_RM_ALLOC, size);
        put(&mut data, CLASS, class);
        data
    }

    fn allocations(device: &Device<FakeRm>) -> usize {
        device.rm.calls.iter().filter(|call| matches!(call, Call::Alloc { .. })).count()
    }

    #[test]
    fn the_marked_layout_gets_its_client_further_along() {
        let mut data = request(ROOT_CLIENT, MARKED_SIZE);
        put(&mut data, LAYOUT_MARK, MARKED_LAYOUT);
        answer_escape(&mut device(), &[], &super::super::tests::NO_MEMORY, &mut data).unwrap();
        assert_eq!(get(&data, RESULT), Some(1));
        assert_eq!(get(&data, MARKED_STATUS), Some(0));
        assert_eq!(get(&data, MARKED_HANDLE), Some(GUEST_CLIENT));
        assert_eq!(get(&data, LAYOUT_MARK), Some(MARKED_LAYOUT), "the mark is not a status");
    }

    #[test]
    fn the_plain_layout_gets_its_client_straight_after_the_status() {
        let mut data = request(ROOT_CLIENT, PLAIN_SIZE);
        answer_escape(&mut device(), &[], &super::super::tests::NO_MEMORY, &mut data).unwrap();
        assert_eq!(get(&data, PLAIN_STATUS), Some(0));
        assert_eq!(get(&data, PLAIN_HANDLE), Some(GUEST_CLIENT));
    }

    #[test]
    fn nothing_the_guest_allocates_reaches_the_rm() {
        let mut device = device();
        let made = allocations(&device);
        for class in [ROOT_CLIENT, SUBDEVICE] {
            let mut data = request(class, PLAIN_SIZE);
            answer_escape(&mut device, &[], &super::super::tests::NO_MEMORY, &mut data).unwrap();
        }
        assert_eq!(allocations(&device), made);
    }

    #[test]
    fn other_classes_keep_the_usual_echo() {
        let mut data = request(SUBDEVICE, PLAIN_SIZE);
        put(&mut data, PLAIN_HANDLE, 0x1234);
        answer_escape(&mut device(), &[], &super::super::tests::NO_MEMORY, &mut data).unwrap();
        assert_eq!(get(&data, PLAIN_HANDLE), Some(0x1234));
    }

    #[test]
    fn a_short_request_is_not_a_problem() {
        let mut data = request(ROOT_CLIENT, CLASS + 4);
        answer_escape(&mut device(), &[], &super::super::tests::NO_MEMORY, &mut data).unwrap();
    }
}
