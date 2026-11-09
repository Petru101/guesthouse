# card-check

Windows guests have only run on an RTX 5090 so far. The 40, 30, 20 and 16 series have profiles now, and nobody has run them on a real card. This program is for anyone with an NVIDIA card on Linux: it reads what the host's driver says about the card, says what Guesthouse would make of it, and then, if you say yes, makes the channels a VM would use and sees whether the card answers. It writes a short text file you can read and then post.

Run `guesthouse-card-check` from a terminal. It needs no root, no Rust, no Windows driver and nothing installed. It writes `guesthouse-card-check.txt` in the folder you ran it from, or in your temp folder if it can't write there, and says where. Nothing is sent anywhere.

```sh
./guesthouse-card-check               # reads the card, asks before the channel test
./guesthouse-card-check --read-only   # stops before the channel test
./guesthouse-card-check --card 1      # with more than one card, picks the second in the list it prints
./guesthouse-card-check --pci 0000:02:00.0   # or picks one by its PCI address
```

## What it reads

First the part that only reads: the driver version and whether it's the open kernel module, each card the driver has, and for the card you picked its name, device ID, architecture, SM version, the engines the host lists, each copy engine's capabilities, the engine nodes a guest would be given, and the class list. Then it says what the app knows: supported, experimental or refused, and why. On an experimental card the decoder and encoder engines in that list are what fills in the node types still unset for its generation.

Then it says what the channel test does and asks for a y or n. The test makes a graphics channel and a copy channel, and on a generation whose video runs one on each video engine, has each write one value to memory, and frees them. That's what a VM does first. On a card nobody has run this on, the GPU could stop answering, which freezes the desktop on that card until a reboot, so save your work first. While each channel is tried, the file says which one, so after a freeze and a reboot it still shows what was read and where it stopped. The objects it makes go when it ends, and nothing on the system changes.

The file never has the card's UUID or ECID, a serial number, your PC's name, your user name or a path in it. The program never asks the card for the first two, and a test proves the file holds none of those shapes.

With more than one card, each is found by its PCI address, which stays the same from boot to boot while the card stays in its slot. The program opens the device file the driver lists for that address, then asks the driver which of its devices is the GPU at that address, as the driver numbers them in the order it set them up, which needn't be the device files' order. It prints and writes the address, the device file and the driver's number it used. This has been tried on pretend cards and not yet on a real PC with two cards. Before the channel test it still compares the name the opened card gives with the one the driver lists at that address, and if they differ it writes that down and stops, as the channels could reach another card.

## When it stops

With no NVIDIA driver loaded, or a driver with no card, it says so. With the proprietary module instead of the open one, or a driver branch this build has no profile for, it writes down what it saw and stops before opening the driver, as the app would.

## Building

`./build.sh` makes `guesthouse-card-check` here, in the image `packaging/Containerfile` describes if you have podman or docker, and otherwise with the Rust on this PC. Either way it refuses a build that needs a newer glibc than Ubuntu 24.04 has, which is the oldest system the app runs on, and one that still names a folder of the PC that built it. `cargo test -p guesthouse-card-check` runs the tests on the pretend Ada and Turing cards, with no card at all.
