#!/usr/bin/env bash
# Verifies the .deb in a CLEAN distro container (driven by scripts/package/linux.ps1 -Verify).
# Expects the package at /pkg/atomik-kb_*.deb. Prints the evidence; exits non-zero on failure.
set -uo pipefail
export DEBIAN_FRONTEND=noninteractive
. /etc/os-release
echo "##### $PRETTY_NAME (glibc $(ldd --version | head -1 | awk '{print $NF}'))"
fail=0
step() { echo; echo "== $*"; }
check() { if "$@"; then echo "OK: $*"; else echo "FAIL: $*"; fail=1; fi; }

echo 'Acquire::ForceIPv4 "true";' > /etc/apt/apt.conf.d/99force-ipv4
apt-get update -qq >/dev/null 2>&1
cp /pkg/atomik-kb_*.deb /tmp/atomik-kb.deb

step "apt install ./atomik-kb_*.deb --no-install-recommends (Depends alone must resolve)"
apt-get install -y --no-install-recommends /tmp/atomik-kb.deb >/tmp/install.log 2>&1; rc=$?
tail -3 /tmp/install.log
check test $rc -eq 0
dpkg -s atomik-kb | grep -E '^(Package|Version|Architecture|Depends|Recommends|Section|Installed-Size)'
echo "-- default install (with Recommends) would add:"
apt-get install -s /tmp/atomik-kb.deb 2>/dev/null | grep -E '^Inst (gnome-keyring|libsecret|gcr)' | head -5

step "dpkg -L"
dpkg -L atomik-kb | grep -v '^/usr/share/icons/hicolor/[0-9x@]*$' 
check test -x /usr/bin/atomik-kb-desktop
check test -x /usr/bin/kb-mcp-bridge
check test -f /usr/share/applications/atomik-kb.desktop
cat /usr/share/applications/atomik-kb.desktop
check grep -q '^Categories=.*Office' /usr/share/applications/atomik-kb.desktop
check test -f /usr/share/icons/hicolor/128x128/apps/atomik-kb-desktop.png

step "ldd: unresolved libraries"
ldd /usr/bin/atomik-kb-desktop | grep -E 'not found' || echo "(no 'not found' entries)"
check test "$(ldd /usr/bin/atomik-kb-desktop | grep -c 'not found')" -eq 0
echo "linked libs: $(ldd /usr/bin/atomik-kb-desktop | wc -l)"

step "install the test tools (xvfb, dbus; no keyring yet)"
apt-get install -y --no-install-recommends xvfb xauth dbus dbus-x11 binutils >/tmp/tools.log 2>&1 || tail -5 /tmp/tools.log
echo "max GLIBC symbol version the binary needs: $(objdump -T /usr/bin/atomik-kb-desktop | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1)"

run_app() { # $1 = label, $2 = shell snippet that prepares the keyring inside the session bus
  local label=$1 pre=$2
  export HOME=/tmp/home-$label; rm -rf "$HOME"; mkdir -p "$HOME"
  export XDG_DATA_HOME=$HOME/.local/share XDG_CONFIG_HOME=$HOME/.config XDG_CACHE_HOME=$HOME/.cache
  dbus-run-session -- bash -c "
    $pre
    xvfb-run -a -s '-screen 0 1280x800x24' bash -c '
      RUST_BACKTRACE=1 /usr/bin/atomik-kb-desktop >/tmp/app-$label.out 2>/tmp/app-$label.err &
      pid=\$!
      sleep 12
      if kill -0 \$pid 2>/dev/null; then echo \"ALIVE after 12s (pid \$pid)\"; kill \$pid; wait \$pid 2>/dev/null; else wait \$pid; echo \"EXITED early, code \$?\"; fi
    '
  " 2>&1 | grep -E '^(ALIVE|EXITED)'
  echo "-- app stderr ($label):"; sed 's/\x1b\[[0-9;]*m//g' /tmp/app-$label.err | head -30
  echo "-- app stdout ($label): $(wc -c < /tmp/app-$label.out) bytes"
  if grep -qiE 'panicked|SIGSEGV|segmentation' /tmp/app-$label.err; then echo "FAIL: panic/crash in $label"; fail=1; else echo "OK: no panic in $label"; fi
  echo "-- app data ($label):"; find "$HOME/.local/share/com.atomik-kb.desktop" -maxdepth 1 | sed "s|$HOME/||"
}

step "launch with NO secret service installed at all"
run_app no-secret-service ':'

step "install gnome-keyring, launch with an unlocked default keyring"
apt-get install -y --no-install-recommends gnome-keyring >/tmp/gk.log 2>&1 || tail -3 /tmp/gk.log
# A login session has a default keyring; reproduce that (the `default` alias file), unlocked.
run_app with-keyring 'mkdir -p $XDG_DATA_HOME/keyrings; echo -n login > $XDG_DATA_HOME/keyrings/default; echo -n test | gnome-keyring-daemon --unlock --components=secrets >/dev/null; gnome-keyring-daemon --start --components=secrets >/dev/null'

step "second launch with the same keyring/data (reopen the existing store)"
HOME=/tmp/home-with-keyring
dbus-run-session -- bash -c '
  export XDG_DATA_HOME=$HOME/.local/share XDG_CONFIG_HOME=$HOME/.config XDG_CACHE_HOME=$HOME/.cache
  echo -n test | gnome-keyring-daemon --unlock --components=secrets >/dev/null; gnome-keyring-daemon --start --components=secrets >/dev/null
  xvfb-run -a bash -c "/usr/bin/atomik-kb-desktop >/tmp/app-second.out 2>/tmp/app-second.err & pid=\$!; sleep 10; kill -0 \$pid && echo \"ALIVE after 10s\"; kill \$pid" ' 2>&1 | grep -E '^(ALIVE|EXITED)'
sed 's/\x1b\[[0-9;]*m//g' /tmp/app-second.err | head -10
grep -qiE 'panicked' /tmp/app-second.err && { echo "FAIL: panic on second launch"; fail=1; } || echo "OK: no panic on second launch"

step "apt remove / purge must not touch user data"
before=$(find /tmp/home-with-keyring -type f -not -path '*/.cache/*' | sort | md5sum)
apt-get remove -y atomik-kb >/tmp/remove.log 2>&1; tail -2 /tmp/remove.log
check test ! -e /usr/bin/atomik-kb-desktop
apt-get purge -y atomik-kb >/dev/null 2>&1
after=$(find /tmp/home-with-keyring -type f -not -path '*/.cache/*' | sort | md5sum)
echo "user-file listing hash before=$before after=$after"
check test "$before" = "$after"
check test -d /tmp/home-with-keyring/.local/share/com.atomik-kb.desktop

echo; [ $fail -eq 0 ] && echo "RESULT $PRETTY_NAME: PASS" || echo "RESULT $PRETTY_NAME: FAIL"
exit $fail
