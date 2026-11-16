#!/usr/bin/env bash
# Runs the receiving side (unit, end-to-end, hand-off and resume tests) with
# its staging and download folders on real filesystems, and the hand-off
# crash test under simulated power cuts (LazyFS discards every write not yet
# synced). Linux only; needs sudo. CI runs it on every build.
#
#   scripts/test-filesystems.sh                 # everything
#   scripts/test-filesystems.sh exfat lazyfs    # some
set -euo pipefail
cd "$(dirname "$0")/.."

FS="${*:-ext4 xfs btrfs vfat exfat ntfs-3g nfs tmpfs lazyfs}"
LAZYFS_COMMIT=3edad01aa936b81fed9bd64e47e725fc5ef6c977
WORK="${RUNNER_TEMP:-/tmp}/warp-fs"
U=$(id -u)
G=$(id -g)
TESTS=(--lib --test e2e --test handoff --test resume)
mkdir -p "$WORK"

cargo test --locked -p libwarp "${TESTS[@]}" --no-run # built outside the mounts

run_on() { # fs mountpoint
  echo "::group::$1 ($(findmnt -no FSTYPE "$2"))"
  TMPDIR="$2" cargo test --locked -p libwarp "${TESTS[@]}"
  echo "::endgroup::"
  CAPS+="$1: $(TMPDIR="$2" cargo test -q --locked -p libwarp --lib fsx::tests::report_capabilities -- --ignored --nocapture |
    sed -n 's/^FS-CAPABILITIES: //p')"$'\n'
}
CAPS=""

image() { # fs → path of a fresh 1 GiB image
  truncate -s 1G "$WORK/$1.img"
  echo "$WORK/$1.img"
}

lazyfs() {
  local src="$WORK/lazyfs-src" m="$WORK/mnt-lazyfs" root="$WORK/lazyfs-root"
  if [ ! -x "$src/lazyfs/build/lazyfs" ]; then
    git clone -q https://github.com/dsrhaslab/lazyfs.git "$src"
    git -C "$src" checkout -q "$LAZYFS_COMMIT"
    (cd "$src/libs/libpcache" && ./build.sh >/dev/null)
    (cd "$src/lazyfs" && ./build.sh >/dev/null)
  fi
  grep -q '^user_allow_other' /etc/fuse.conf || echo user_allow_other | sudo tee -a /etc/fuse.conf >/dev/null
  rm -f "$WORK/faults.fifo" "$WORK/done.fifo"
  mkfifo "$WORK/faults.fifo" "$WORK/done.fifo"
  # LazyFS opens done.fifo for writing once, at start-up, and reports each
  # finished command with one write. Without a reader open at that instant the
  # report is lost (EPIPE) and the wait below never ends. This reader, held
  # open for the whole run, keeps every report buffered in the pipe.
  exec 9<>"$WORK/done.fifo"
  cat >"$WORK/lazyfs.toml" <<EOF
[faults]
fifo_path="$WORK/faults.fifo"
fifo_path_completed="$WORK/done.fifo"
[cache]
apply_eviction=false
[cache.simple]
custom_size="1gb"
blocks_per_page=1
[filesystem]
log_all_operations=false
logfile=""
EOF
  mkdir -p "$m" "$root"
  "$src/lazyfs/build/lazyfs" "$m" --config-path "$WORK/lazyfs.toml" -o allow_other -o modules=subdir -o subdir="$root" -f \
    >"$WORK/lazyfs.log" 2>&1 &
  for _ in $(seq 50); do findmnt "$m" >/dev/null && break; sleep 0.2; done
  findmnt "$m" >/dev/null || { cat "$WORK/lazyfs.log"; exit 1; }

  local cut="echo lazyfs::clear-cache > '$WORK/faults.fifo' && timeout 60 head -n1 '$WORK/done.fifo' >/dev/null"
  # Sanity: the power cut really discards unsynced data, and keeps synced data.
  local probe="$m/probe"
  echo lost >"$probe.lost"
  echo kept >"$probe.kept" && python3 -c "import os,sys; f=os.open(sys.argv[1], os.O_RDONLY); os.fsync(f)" "$probe.kept"
  sh -c "$cut"
  [ "$(cat "$probe.kept")" = kept ] || { echo "LazyFS lost synced data"; exit 1; }
  [ "$(cat "$probe.lost" 2>/dev/null)" != lost ] || { echo "LazyFS did not discard unsynced data"; exit 1; }
  rm -f "$probe".*

  echo "::group::power cuts at every hand-off step (LazyFS)"
  TMPDIR="$m" WARP_POWER_CUT="$cut" cargo test --locked -p libwarp --lib receiver::tests::a_crash_at
  echo "::endgroup::"

  # And the test notices when a sync is missing: drop the chunk log's.
  echo "::group::power-cut test catches a missing sync"
  local rs=core-rust/libwarp/src/receiver.rs
  cp "$rs" "$WORK/receiver.rs.orig"
  sed -i 's/fsx::sync_data(&log)/Ok::<(), std::io::Error>(())/' "$rs"
  ! cmp -s "$rs" "$WORK/receiver.rs.orig" || { echo "mutation did not apply"; exit 1; }
  if TMPDIR="$m" WARP_POWER_CUT="$cut" cargo test --locked -p libwarp --lib receiver::tests::a_crash_at; then
    cp "$WORK/receiver.rs.orig" "$rs"
    echo "power-cut test passed without the chunk-log sync"
    exit 1
  fi
  cp "$WORK/receiver.rs.orig" "$rs"
  echo "::endgroup::"
}

for fs in $FS; do
  m="$WORK/mnt-$fs"
  mkdir -p "$m"
  case $fs in
    ext4) sudo mkfs.ext4 -q -F "$(image $fs)" && sudo mount -o loop "$WORK/$fs.img" "$m" && sudo chown "$U:$G" "$m" ;;
    xfs | btrfs) sudo "mkfs.$fs" -q -f "$(image $fs)" && sudo mount -o loop "$WORK/$fs.img" "$m" && sudo chown "$U:$G" "$m" ;;
    vfat) sudo mkfs.vfat -F 32 "$(image $fs)" >/dev/null && sudo mount -o loop,uid="$U",gid="$G" "$WORK/$fs.img" "$m" ;;
    exfat) sudo mkfs.exfat "$(image $fs)" >/dev/null && sudo mount -o loop,uid="$U",gid="$G" "$WORK/$fs.img" "$m" ;;
    ntfs-3g) # FUSE
      sudo mkfs.ntfs -Q -F "$(image $fs)" >/dev/null && sudo ntfs-3g -o uid="$U",gid="$G" "$WORK/$fs.img" "$m" ;;
    nfs)
      sudo mkdir -p /srv/warp-nfs && sudo chown "$U:$G" /srv/warp-nfs
      echo "/srv/warp-nfs 127.0.0.1(rw,sync,no_subtree_check)" | sudo tee /etc/exports >/dev/null
      sudo systemctl start nfs-kernel-server && sudo exportfs -ra
      sudo mount -t nfs -o vers=4.2 127.0.0.1:/srv/warp-nfs "$m" ;;
    tmpfs) sudo mount -t tmpfs -o size=1G,uid="$U",gid="$G" tmpfs "$m" ;;
    lazyfs) lazyfs; continue ;;
    *) echo "unknown filesystem $fs"; exit 2 ;;
  esac
  run_on "$fs" "$m"
done
echo "all filesystems passed: $FS"
printf '\nHow each one avoids replacing an existing name:\n%s' "$CAPS"
