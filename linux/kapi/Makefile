# Needs NVIDIA's public headers at the host driver's version: NV is open-gpu-kernel-modules. None are copied here.
NV ?= /usr/src/open-gpu-kernel-modules
obj-m := nvidia-modeset.o
nvidia-modeset-y := rm.o memory.o flips.o device.o
ccflags-y := -I$(NV)/kernel-open/common/inc -I$(NV)/src/nvidia-modeset/kapi/interface -I$(NV)/src/nvidia-modeset/interface \
	-I$(NV)/src/common/sdk/nvidia/inc -I$(NV)/src/common/modeset/timing -I$(NV)/src/common/unix/common/inc -I$(NV)/src/common/modeset/hdmipacket -I$(NV)/src/common/inc -I$(NV)/src/nvidia/arch/nvalloc/unix/include -I$(src)/../guest
KBUILD_EXTRA_SYMBOLS := $(CURDIR)/../guest/Module.symvers
KDIR ?= /lib/modules/$(shell uname -r)/build
all:
	$(MAKE) -C $(KDIR) M=$(CURDIR) modules
