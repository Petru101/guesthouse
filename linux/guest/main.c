// SPDX-License-Identifier: GPL-2.0-or-later
/* Finds the nvfwd device, and makes the device nodes and /proc/driver/nvidia. */
#include <linux/cdev.h>
#include <linux/device.h>
#include <linux/io.h>
#include <linux/module.h>
#include <linux/proc_fs.h>
#include <linux/seq_file.h>
#include "nvfwd_guest.h"

#define NV_MAJOR 195
/* The guest build passes the version of the host driver it was built for. */
#ifndef NV_VERSION
#define NV_VERSION "unknown"
#endif

static int proc_version(struct seq_file *m, void *v)
{
	seq_puts(m, "NVRM version: NVIDIA UNIX Open Kernel Module for x86_64  " NV_VERSION "  Forwarded\n"
		    "GCC version:  forwarded\n");
	return 0;
}

static int proc_params(struct seq_file *m, void *v)
{
	seq_puts(m, "ModifyDeviceFiles: 0\nDeviceFileUID: 0\nDeviceFileGID: 0\nDeviceFileMode: 438\n");
	return 0;
}

static struct proc_dir_entry *proc_root;

static void proc_setup(void)
{
	proc_root = proc_mkdir("driver/nvidia", NULL);
	if (!proc_root)
		return;
	proc_create_single("version", 0444, proc_root, proc_version);
	proc_create_single("params", 0444, proc_root, proc_params);
}

/* ---- probe ---- */

static char *nv_devnode(const struct device *dev, umode_t *mode)
{
	if (mode)
		*mode = 0666;
	return NULL;
}

static struct cdev cdev_nv, cdev_uvm;

static int nvf_probe(struct pci_dev *pdev, const struct pci_device_id *id)
{
	dev_t uvm_dev;
	int rc;

	nv.fops = &nvf_fops;
	rc = pcim_enable_device(pdev);
	if (rc)
		return rc;
	nv.mmio = pci_iomap(pdev, 0, 0);
	nv.slots = memremap(pci_resource_start(pdev, 2), pci_resource_len(pdev, 2), MEMREMAP_WB);
	nv.win_base = pci_resource_start(pdev, 4);
	nv.win_size = pci_resource_len(pdev, 4);
	nv.dev = &pdev->dev;
	if (!nv.mmio || !nv.slots || !nv.win_base)
		return -ENODEV;
	if (readl(nv.mmio) != NVFWD_MAGIC)
		return -ENODEV;
	dev_info(&pdev->dev, "window %llx size %llx\n", (u64)nv.win_base, nv.win_size);
	sema_init(&nv.free, NVFWD_SLOTS);
	spin_lock_init(&nv.lock);
	init_waitqueue_head(&nv.pollq);
	hrtimer_setup(&nv.poll_timer, poll_tick, CLOCK_MONOTONIC, HRTIMER_MODE_REL);

	rc = register_chrdev_region(MKDEV(NV_MAJOR, 0), 256, "nvidia-frontend");
	if (rc)
		return rc;
	cdev_init(&cdev_nv, &nvf_fops);
	rc = cdev_add(&cdev_nv, MKDEV(NV_MAJOR, 0), 256);
	if (rc)
		return rc;
	rc = alloc_chrdev_region(&uvm_dev, 0, 2, "nvidia-uvm");
	if (rc)
		return rc;
	nv.uvm_major = MAJOR(uvm_dev);
	cdev_init(&cdev_uvm, &nvf_fops);
	rc = cdev_add(&cdev_uvm, uvm_dev, 2);
	if (rc)
		return rc;
	nv.cls = class_create("nvidia");
	if (IS_ERR(nv.cls))
		return PTR_ERR(nv.cls);
	nv.cls->devnode = nv_devnode;
	device_create(nv.cls, NULL, MKDEV(NV_MAJOR, 255), NULL, "nvidiactl");
	device_create(nv.cls, NULL, MKDEV(NV_MAJOR, 0), NULL, "nvidia0");
	device_create(nv.cls, NULL, MKDEV(NV_MAJOR, 254), NULL, "nvidia-modeset");
	device_create(nv.cls, NULL, uvm_dev, NULL, "nvidia-uvm");
	proc_setup();
	return 0;
}

static const struct pci_device_id nvf_ids[] = {
	{ PCI_DEVICE(0x1234, 0x11ea) },
	{ }
};
MODULE_DEVICE_TABLE(pci, nvf_ids);

static struct pci_driver nvf_driver = {
	.name = "nvfwd",
	.id_table = nvf_ids,
	.probe = nvf_probe,
};

static int __init nvf_init(void)
{
	return pci_register_driver(&nvf_driver);
}
module_init(nvf_init);

MODULE_VERSION(NV_VERSION);
