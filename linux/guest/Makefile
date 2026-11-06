# The tables come from NVIDIA's source at the host driver's version: the app makes them with a driver image.
obj-m := nvidia.o
nvidia-y := slots.o call.o rm.o uvm.o show.o
ccflags-y := -I$(src)
ifdef NV_VERSION
ccflags-y += -DNV_VERSION=\"$(NV_VERSION)\"
endif
KDIR ?= /lib/modules/$(shell uname -r)/build
all:
	$(MAKE) -C $(KDIR) M=$(CURDIR) modules
