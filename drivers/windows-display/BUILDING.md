# Building and installing the display driver

You build this in Windows, and sign it on the machine that will run it. Nothing here gives you a signed driver, because I don't have a certificate Microsoft would accept, and I'm not sure this driver could get one.

## What you need

- A Windows 11 machine or VM to build in. It doesn't need a GPU.
- The Enterprise WDK ISO, from [Microsoft's download page](https://learn.microsoft.com/en-us/windows-hardware/drivers/download-the-wdk). It has the compiler, the SDK and the WDK in one file, so there's nothing to install. You accept Microsoft's licence yourself when you download it. I can't include it here, and don't put it in this repo.

## Building

Mount the ISO, run `LaunchBuildEnv.cmd` from it, and in that window, from this folder:

    msbuild ZakoVDD.sln /m /p:Configuration=Release /p:Platform=x64 /p:SignMode=Off

The driver is in `x64\Release\ZakoVDD`. You want `ZakoVDD.dll` and `ZakoVDD.inf` from there. The catalog file that's beside them gets replaced in the next step.

Warnings are not errors in this project, as newer compilers flag zako's code in places I haven't touched. My files build with none.

## Installing

In the Windows guest, make a folder with `vdd_settings.xml` in it, and a `pkg` folder inside holding the DLL and INF. Then, in PowerShell as an administrator:

    setup\sign-and-install.ps1 C:\the\folder

After that, so that the screen comes up by itself:

    setup\install-take-over.ps1 C:\where\take-over-display.ps1\is 1920 1080 60 YourUserName

That runs `take-over-display.ps1` when you log on, with your size and refresh rate. Copy it somewhere it'll stay first.

## What the certificate means

Windows won't install a driver package unless someone it trusts signed the catalog. So `sign-and-install.ps1` does this on your machine:

1. Makes a new certificate, just for this machine.
2. Puts it in the machine's Trusted Root and Trusted Publishers stores.
3. Signs the driver's catalog with it.
4. Deletes the private key.

Step 2 is the part to think about. A trusted root is powerful: while its key exists, it could vouch for anything on that machine. That's why step 4 is there. The key can't be exported, it's deleted before the install, and nothing is ever signed with it again. So nobody has that key afterwards, me included, and a new build means a new certificate.

Every install makes its own certificate, so there's nothing shared between machines to leak.

If you want it gone, remove the driver with `pnputil`, then delete the certificate named "Guesthouse display driver, this machine only" from both stores in `certlm.msc`.

This has only ever been done in a VM, and that's what it's for. I wouldn't run it on a machine that isn't a guest.

## Secure Boot

You can leave it on. I installed this with Secure Boot on, and it loaded and showed the right picture on that boot and the next, with no test signing and no warnings in Windows' Code Integrity log. The driver runs in user mode, so there's no kernel driver of ours for Secure Boot to refuse.

What I haven't tried:

- Memory integrity. It was off in my test.
- Smart App Control turned on. It was in evaluation mode in my test, and a self-signed driver could well upset it.
- Anti-cheat software, which may not like a home-made root certificate. I don't know.
