# card-report

Windows guests only run on Blackwell so far. To add another card I need to know what NVIDIA's Windows driver reports for it, and that only shows up on real Windows. This program writes that down in a short text file you can read and then post.

Run `card-report.exe`, by double-click or from a terminal. It needs no admin rights. It writes `guesthouse-card-report.txt` in the folder you ran it from, or in your temp folder if it can't write there, and says where. Nothing is sent anywhere.

## What it reads

It asks Windows the ordinary adapter questions any program using the GPU asks, through `D3DKMTEnumAdapters2` and `D3DKMTQueryAdapterInfo`. It changes no settings, installs nothing and sends nothing to the driver.

From Windows:

- the Windows version
- the GPU's name, device ID and PCI revision
- the driver version and WDDM version
- the engine nodes: type and name

From the driver's record:

- its size and format
- architecture, implementation and chip revision
- SM version and shader set
- video memory sizes
- for each engine node: its type, scheduling, how much can be in flight, and its copy engine
- the class list

That record also holds things that identify your card and PC, like the card's UUID and ECID. The program reads the fields above by name and looks at nothing else in it. The record itself never leaves memory, and there are no hex dumps. If the record's size isn't one the program has a layout for, or the card's own ID isn't where the layout says, it reads nothing from the record and the report says so.

It knows two layouts, 617 and 621. The 621 one hasn't met a real record yet, and the report says that. On 621 the fields that moved (name, memory, nodes, classes) are only kept if every one of them looks right, and otherwise the report says they can't be read.

The file never has your PC's name, your user name, serial numbers or paths in it.

## Building

`./build.sh` makes `card-report.exe` with mingw, in a container if you don't have mingw. `./test.sh` runs the tests on Linux. One of them changes every byte of a made-up record in turn, to prove only the named fields can reach the report.

A new driver layout is a new row in `layouts` in `record.c`.
