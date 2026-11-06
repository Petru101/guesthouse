# What ends up where.

t_a_glibc_too_old_is_said_plainly() {
    release 1.0
    give_everything
    printf '#!/bin/sh\necho glibc 2.20\n' >"$T/bin/getconf.new"
    chmod +x "$T/bin/getconf.new"
    mv -f "$T/bin/getconf.new" "$T/bin/getconf"
    snapshot >"$T/before"
    install_version 1.0
    check_exit 1
    check_said "needs glibc 2.30"
    check_home_is "$T/before"
}


# A source bundle whose build script just packs what make-release made, to prove the path around it.
