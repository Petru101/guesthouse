//! Opening the card setup chose on a PC with two, the way the daemon does: the driver's list gives the
//! device file for the address, and the RM is asked which of its devices that is. Here the RM set the
//! cards up in the other order from their device files, as it can on a real PC. Nobody has run this
//! on a real PC with two cards yet: these are pretend ones.

use guesthouse_backend::PciAddress;
use guesthouse_backend_nvidia::profile::HOST_615;
use guesthouse_backend_nvidia::rm::device::Device;
use guesthouse_backend_nvidia::rm::fake::{FakeGpu, FakeRm};
use guesthouse_backend_nvidia::rm::listing::{CardPlace, ListedCard, place_for};

fn at(text: &str) -> PciAddress {
    PciAddress::parse(text).unwrap()
}

fn listing() -> Vec<ListedCard> {
    let card = |address: &str, minor: u32, model: &str| ListedCard {
        address: at(address),
        minor,
        model: model.to_owned(),
    };
    vec![
        card("0000:01:00.0", 0, "NVIDIA GeForce RTX 4080"),
        card("0000:02:00.0", 1, "NVIDIA GeForce GTX 1660 SUPER"),
    ]
}

/// What the RM has: the 1660 SUPER was set up first, so it is device instance 0.
fn rm_opened_at(place: &CardPlace) -> FakeRm {
    let mut rm = FakeRm::with_gpus(vec![
        FakeGpu { id: 0x0100, address: at("0000:01:00.0"), minor: 0, device_instance: 1, attached: false },
        FakeGpu { id: 0x0200, address: at("0000:02:00.0"), minor: 1, device_instance: 0, attached: true },
    ]);
    rm.open_device_file(place.minor);
    rm
}

#[test]
fn each_card_setup_chose_is_opened_by_its_own_instance() {
    for (address, minor, instance) in [("0000:01:00.0", 0, 1), ("0000:02:00.0", 1, 0)] {
        let place = place_for(&listing(), Some(&at(address))).unwrap();
        assert_eq!(place, CardPlace { minor, address: Some(at(address)) });
        let device = Device::open_place(rm_opened_at(&place), &HOST_615, &place).unwrap();
        assert_eq!(device.instance, instance, "{address}");
    }
}

#[test]
fn the_old_way_reaches_the_other_card_and_is_refused() {
    let place = place_for(&listing(), Some(&at("0000:01:00.0"))).unwrap();
    let error = Device::open(rm_opened_at(&place), &HOST_615).err().unwrap();
    assert!(error.to_string().contains("status 0x1b"), "{error}");
}

#[test]
fn two_cards_and_no_address_is_refused_before_anything_opens() {
    assert!(place_for(&listing(), None).unwrap_err().contains("Run Setup Again"));
}

#[test]
fn one_card_opens_as_it_always_did() {
    let only = &listing()[..1];
    for wanted in [None, Some(at("0000:01:00.0"))] {
        let place = place_for(only, wanted.as_ref()).unwrap();
        let device = Device::open_place(FakeRm::new(), &HOST_615, &place).unwrap();
        assert_eq!((place.minor, device.instance), (0, 0));
    }
}
