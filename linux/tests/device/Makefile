# Tests of the nvfwd device's checks that need no GPU and no running QEMU, under ASan and UBSan.
# QEMU is a source folder with qemu/qemu-gpupv.patch applied. BUILD is where the programs go.
QEMU ?= $(error set QEMU to a patched QEMU source folder)
BUILD ?= build
DEVICE = $(QEMU)/hw/misc/nvfwd
CFLAGS := -O1 -g -Wall -Wextra -fsanitize=address,undefined -fno-sanitize-recover=all -D_GNU_SOURCE
GLIB := $(shell pkg-config --cflags --libs glib-2.0)
TESTS := test_runs test_callrate test_objects test_maps test_bounce test_rmargs test_clients test_accounting

all: $(TESTS:%=$(BUILD)/%) $(BUILD)/test_sensors $(BUILD)/fake_sensord
	@# The device's tables live as long as the VM and have no free, so a leak report means nothing here.
	@for t in $(TESTS:%=$(BUILD)/%); do echo "== $$t"; ASAN_OPTIONS=detect_leaks=0 $$t || exit 1; done
	@echo "== test_sensors"
	@ASAN_OPTIONS=detect_leaks=0 $(BUILD)/test_sensors $(BUILD)/fake_sensord

$(BUILD)/test_runs: test_runs.c
	@mkdir -p $(BUILD)
	$(CC) $(CFLAGS) -I$(DEVICE) -o $@ $< $(DEVICE)/runs.c
$(BUILD)/test_callrate: test_callrate.c
	@mkdir -p $(BUILD)
	$(CC) $(CFLAGS) -I$(DEVICE) -o $@ $< $(DEVICE)/callrate.c
$(BUILD)/test_objects: test_objects.c
	@mkdir -p $(BUILD)
	$(CC) $(CFLAGS) -I$(DEVICE) $(GLIB) -o $@ $< $(DEVICE)/objects.c $(GLIB)
$(BUILD)/test_maps: test_maps.c
	@mkdir -p $(BUILD)
	$(CC) $(CFLAGS) -I$(DEVICE) $(GLIB) -o $@ $< $(DEVICE)/maps.c $(GLIB)
$(BUILD)/test_bounce: test_bounce.c
	@mkdir -p $(BUILD)
	$(CC) $(CFLAGS) -I$(DEVICE) -o $@ $< $(DEVICE)/bounce.c
$(BUILD)/test_rmargs: test_rmargs.c
	@mkdir -p $(BUILD)
	$(CC) $(CFLAGS) -I$(DEVICE) -o $@ $< $(DEVICE)/rmargs.c
$(BUILD)/test_clients: test_clients.c
	@mkdir -p $(BUILD)
	$(CC) $(CFLAGS) -I$(DEVICE) -o $@ $< $(DEVICE)/clients.c $(DEVICE)/rmargs.c
$(BUILD)/test_accounting: test_accounting.c
	@mkdir -p $(BUILD)
	$(CC) $(CFLAGS) -I$(DEVICE) $(GLIB) -o $@ $< $(DEVICE)/accounting.c $(DEVICE)/objects.c $(GLIB)
$(BUILD)/test_sensors: test_sensors.c
	@mkdir -p $(BUILD)
	$(CC) $(CFLAGS) -Istand-in -I$(DEVICE) $(GLIB) -o $@ $< $(DEVICE)/sensors.c $(GLIB)
$(BUILD)/fake_sensord: fake_sensord.c
	@mkdir -p $(BUILD)
	$(CC) $(CFLAGS) -I$(DEVICE) -o $@ $<

.PHONY: all
