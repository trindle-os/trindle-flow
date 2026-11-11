#!/usr/bin/env bash
# Builds Flow as a Trindle plugin (interface 1): the release program, the
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(dirname "$here")"
out="${1:-$PWD/target/plugin}"
id="org.trindle.Flow"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)"
plugin="$out/$id"

cargo_home=${CARGO_HOME:-$HOME/.cargo}
target="$root/target/shipped"
RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$HOME=/home --remap-path-prefix=$(dirname "$root")=/src --remap-path-prefix=$cargo_home=/cargo --remap-path-prefix=$target=/target"
export RUSTFLAGS
locked=--locked
if [ -f "$root/.cargo/config.toml" ] && grep -q '^\[patch' "$root/.cargo/config.toml"; then locked=; fi
cargo +1.85 build --release --offline $locked --manifest-path "$root/Cargo.toml" --target-dir "$target"
if strings -a "$target/release/flow" | grep -q -E "$(basename "$HOME")|\.cargo|rustup|target/release/build"; then
    echo "the built program still holds a path from this computer" >&2
    exit 1
fi

needs="$(ldd "$target/release/flow" | awk '/=>/ {print $1}' | sort | tr '\n' ' ')"
if [ "$needs" != "libc.so.6 libgcc_s.so.1 " ] && [ "$needs" != "libc.so.6 libgcc_s.so.1 libm.so.6 " ]; then
    echo "flow now needs other libraries ($needs): carry them in lib/ before shipping" >&2
    exit 1
fi

rm -rf "$plugin" "$out/$id.tar.zst"
install -d "$plugin/bin" "$plugin/icons"
install -m 0755 "$target/release/flow" "$plugin/bin/flow"
install -m 0644 "$here/icons/flow-ring.svg" "$plugin/icons/flow-ring.svg"
install -m 0644 "$root/LICENSE" "$plugin/LICENSE"
install -d "$plugin/share/licences"
install -m 0644 "$root/share/licences/THIRD-PARTY.txt" "$plugin/share/licences/THIRD-PARTY.txt"

cat > "$plugin/plugin.ini" <<INI
[Plugin]
Interface=1
Id=$id
Name=Flow
Version=$version
Summary=Use one computer's mouse and keyboard on another, with the clipboard and files shared.
Permissions=network;listen;files;notifications;background;control;capture;copied
Shortcuts=switch

[Shortcut switch]
Label=Switch computer
Keys=Ctrl+Alt+S
Run=bin/flow switch

[Ring]
Icon=icons/flow-ring.svg
Label=Flow
Run=bin/flow tell
Menu=home;connect;disconnect

[Menu home]
Label=Bring the pointer back
Run=bin/flow home

[Menu connect]
Label=Connect
Run=bin/flow connect

[Menu disconnect]
Label=Disconnect
Run=bin/flow disconnect

[PinUnlock]
Run=bin/flow unlock

[Network]
Listen=tcp:24810;udp:24811

[Settings]
Fields=state;controls;use_computer;allow_computer;allow_number;other_side;shortcut_only;left_computer;right_computer;top_computer;bottom_computer;pointer_speed;address;discovery;share_clipboard;share_files;files_keep;files_folder

[Field state]
Kind=note
TextFrom=status.txt

[Field controls]
Kind=switch
Label=This computer controls the other one
Default=false

[Field use_computer]
ShowWhen=controls=false
Kind=choice
Label=Computer that controls this one
ChoicesFrom=found.txt

[Field allow_computer]
ShowWhen=controls=true
Kind=choice
Label=Computer asking to be controlled
ChoicesFrom=asking.txt

[Field allow_number]
ShowWhen=controls=true
Kind=text
Label=Number that computer shows

[Field other_side]
ShowWhen=controls=true
Kind=choice
Label=The other computer is
Choices=Left;Right;Above;Below
Default=Right

[Field shortcut_only]
ShowWhen=controls=true
Kind=switch
Label=Cross only by shortcut or menu, not by pushing
Default=false

[Field left_computer]
ShowWhen=controls=true
Kind=choice
Label=Computer on the left
ChoicesFrom=computers.txt

[Field right_computer]
ShowWhen=controls=true
Kind=choice
Label=Computer on the right
ChoicesFrom=computers.txt

[Field top_computer]
ShowWhen=controls=true
Kind=choice
Label=Computer above
ChoicesFrom=computers.txt

[Field bottom_computer]
ShowWhen=controls=true
Kind=choice
Label=Computer below
ChoicesFrom=computers.txt

[Field pointer_speed]
ShowWhen=controls=true
Kind=choice
Label=Pointer speed on the other computer
Choices=Slower;Normal;Faster;Much faster
Default=Normal

[Field address]
ShowWhen=controls=false
Kind=text
Label=Address of the other computer, for Tailscale

[Field discovery]
Kind=switch
Label=Look for the other computer on this network
Default=true

[Field share_clipboard]
Kind=switch
Label=Share the clipboard with the other computer
Default=true

[Field share_files]
Kind=switch
Label=Receive files copied on the other computer
Default=true

[Field files_keep]
Kind=switch
Label=Keep received files in Downloads, Flow
Default=true

[Field files_folder]
ShowWhen=files_keep=true
Kind=text
Label=Folder for received files, if not Downloads

[Background]
Run=bin/flow run
INI

install -m 0644 "$here/icons/flow.svg" "$out/$id.svg"
cat > "$out/$id.description.txt" <<TXT
Flow lets one computer's mouse and keyboard work another computer on your network. Install it on both. On the one whose mouse and keyboard you will use, switch on "This computer controls the other one" in Flow's settings and say which side the other computer is on. On the other, Flow lists the computers it has found: pick the controlling one and it shows a four-digit number; on the controlling one, pick the computer asking and type that number. From then on they find each other by themselves, over an encrypted link that trusts only the paired computer. Push the pointer off that side of the screen and it carries on on the other computer, with the keyboard following; push it back to come home. What you copy on one can be pasted on the other, and copied files arrive in Downloads, in a Flow folder.
TXT
tar --zstd -cf "$out/$id.tar.zst" -C "$out" "$id"
echo "$plugin"
