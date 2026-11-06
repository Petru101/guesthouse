# Cutting a release

I do this by hand on my own PC. Nothing is automated yet, and `SHA256SUMS` isn't signed yet.

The order matters. `install.sh` on `main` is what points people at a release, so it changes last, once everything it names is really there.

Below, `VERSION` is the version with no `v`, like `0.2.0`, and the build folder is anywhere with about 10 GB free.

1. **Set the version.** Change `version` in `Cargo.toml`, run `cargo check` so `Cargo.lock` follows, and commit that to `main`.

2. **Tag it.** The download URLs have a `v` in them, so the tag must too.

   ```sh
   git tag vVERSION
   ```

3. **Build the display driver.** This one step is in Windows, as [drivers/windows-display/BUILDING.md](drivers/windows-display/BUILDING.md) says, from the tagged commit. Copy `GuesthouseDisplay.dll` and `GuesthouseDisplay.inf` to a folder on this PC, outside the repo. They never go in git.

4. **Build it in the images.** It needs podman or docker and no root, and takes a commit with nothing uncommitted. It stops if the release needs a glibc above 2.39, if the bundled source is missing, or if it has no display driver. The driver's licences go in beside it.

   ```sh
   packaging/build-in-images.sh --out BUILD_FOLDER --display-driver DRIVER_FOLDER
   ```

   The last lines it prints are the version and two hashes for step 7.

5. **Try it.** Both of these need the release in `BUILD_FOLDER`, and neither touches your real home folder.

   ```sh
   tests/install/run.sh
   tests/install/real-run.sh BUILD_FOLDER NEW_SCRATCH_FOLDER
   tests/install/real-in-containers.sh BUILD_FOLDER
   ```

6. **Upload it.** Push the tag, then make the release with these four files, and no others. Nothing points at it yet, so a mistake here costs nothing.

   ```sh
   git push origin main vVERSION
   gh release create vVERSION --title VERSION --notes-file NOTES \
     BUILD_FOLDER/guesthouse-VERSION-x86_64.tar.xz \
     BUILD_FOLDER/guesthouse-VERSION-source.tar.xz \
     BUILD_FOLDER/guesthouse-VERSION-bundled-source.tar \
     BUILD_FOLDER/SHA256SUMS
   ```

   The two sources are there for the GPL and the LGPL, so a release doesn't go out without them. They have to stay up for as long as the payload does.

7. **Point `main` at it, last of all.** In `install.sh`, set `RELEASE_VERSION`, `PAYLOAD_SHA256` and `SOURCE_SHA256` to what step 4 printed, then commit and push. From that moment the one line installs this release, and `--update` and `--check-update` offer it to everyone.

8. **Check it from the outside.** Run `install.sh --dry-run` from the raw URL, in a fake HOME. It should download, verify and show the plan.

If something's wrong after step 7, revert that one commit, and everyone goes back to the release before. A file on a release must never change once `main` names it: fix it with a new version.
