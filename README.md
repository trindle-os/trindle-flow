<p align="center"><img src="docs/images/banner.png" alt="Flow: the Flow mark, the name, the words one mouse and keyboard for two computers, and the chips mouse and keyboard, clipboard and files" width="100%"></p>

<p align="center">
<img src="https://img.shields.io/badge/version-2.6.4-fbc711?style=flat-square&labelColor=111827" alt="Version 2.6.4">
<img src="https://img.shields.io/badge/licence-GPL--3.0--or--later-8aa0c8?style=flat-square&labelColor=111827" alt="Licence GPL 3.0 or later">
<img src="https://img.shields.io/badge/installed%20from-Trindle%20Apps-fbc711?style=flat-square&labelColor=111827" alt="Installed from Trindle Apps">
<img src="https://img.shields.io/badge/cloud-none-8aa0c8?style=flat-square&labelColor=111827" alt="Cloud: none">
</p>

Flow is a Trindle plugin. It lets one computer's mouse and keyboard work
another computer on your network, and shares the clipboard and copied files
between the two. Either computer can be the one whose mouse and keyboard
are used.

The two computers talk to each other directly, over an encrypted link that
trusts only the computer you paired. There is no account, no server in
between and nothing sent outside your network. Holding Ctrl, Alt and Shift
for a second always brings the mouse and keyboard home.

The full help, with every setting and what to do when something goes
wrong, is in [docs/HELP.md](docs/HELP.md).

Comments are stripped from the published code to keep it lean.

## How it works

<p align="center"><img src="docs/images/how-it-works.png" alt="Three steps left to right: install Flow on both computers, pair them with a four-digit number, push the pointer across" width="100%"></p>

## Quick start

Install Flow on both computers from Apps (the Plugins page), and allow what
it asks for. Then, in Flow's settings:

- On the computer whose mouse and keyboard you want to use, switch on
  **This computer controls the other one**, and under **The other computer
  is** choose the side of its screen the other computer sits on.
- On the other computer, leave that switch off.

Then pair them. The computer to be controlled lists the controlling
computers Flow has found, under **Computer that controls this one**: pick
yours, and it shows a four-digit number. The controlling computer then lists
it under **Computer asking to be controlled**: pick it there and type the
number under **Number that computer shows**.

The one thing that trips people up: the number is typed on the controlling
computer, not on the one showing it. Once paired, the two find each other on
the local network by themselves. Picking Nobody unpairs them.

## Using it

Push the pointer off the chosen side of the screen and it carries on on the
other computer, with the keyboard following it. Push it back against the
side facing home to come back. Moving along an edge or brushing it never
crosses, and crossing never opens the ring. Ctrl+Alt+S switches without
pushing.

Whatever happens, holding Ctrl, Alt and Shift together for a second brings
the mouse and keyboard back. So does the screen locking, and Flow stopping.
If the other computer stops answering, the pointer comes back by itself.

## Clipboard and files

What you copy on one computer can be pasted on the other. Copied files
arrive in Downloads, in a Flow folder. A file that stops part way is said to
be lost and its half is removed, and a copy with a lost file is never
pasted.

## Unlocking the other computer

The controlling computer can unlock the other one's lock screen with your
PIN, never the password. Flow asks for the PIN each time, carries it over
its own link to the locked computer and nowhere else, and wipes its own
copies once it has been tried, as far as it can. That computer's Trindle checks it and counts the
tries: after five wrong ones only the password unlocks it. Choose "Unlock"
and the computer's name in Flow's ring menu; Trindle shows the PIN box.

## What it does

- Shares one computer's mouse and keyboard with another, on any side of the
  screen.
- Shares the clipboard both ways.
- Carries copied files across, into Downloads, Flow.
- Unlocks the other computer's lock screen with your PIN.
- Pairs with a four-digit number that is never sent.
- Finds the paired computer on the local network by itself.
- Disconnects and connects again from the ring menu, staying paired.

## Settings

These are on Flow's settings page. The last column is how to change the
same thing from the command line.

| Setting | Default | What it does | Command |
|---|---|---|---|
| This computer controls the other one | off | On: this computer's mouse and keyboard are shared. Off: this computer is the one controlled. | `flow set role control` |
| Computer that controls this one | none | On the controlled computer, the computer to pair with. Nobody unpairs. | `flow use ID` |
| Computer asking to be controlled | none | On the controlling computer, the computer to let pair. | `flow allow ID` |
| Number that computer shows | none | The four-digit number the computer asking shows. | `flow allow ID`, number on standard input |
| The other computer is | Right | Which side of this screen the pointer leaves by. | `flow set other_side Left` |
| Cross only by shortcut or menu, not by pushing | off | On: pushing against the side never crosses. | `flow set shortcut_only on` |
| Computer on the left, right, above, below | none | With more than one other computer, which one sits on each side. | `flow set left_computer ID` |
| Pointer speed on the other computer | Normal | Slower, Normal, Faster or Much faster. | `flow set pointer_speed Faster` |
| Share the clipboard with the other computer | on | Copy on one, paste on the other. | `flow set share_clipboard off` |
| Receive files copied on the other computer | on | Copied files come across. | `flow set share_files off` |
| Keep received files in Downloads, Flow | on | Off: received files go to a temporary folder and are not kept. | `flow set files_keep off` |
| Address of the other computer, for Tailscale | none | On the controlled computer, the address to use when the other computer is not found on the local network. | `flow set address 100.64.0.7` |
| Look for the other computer on this network | on | Off: Flow neither looks nor answers, so the controlled computer needs the address above. Changing it restarts the link. | `flow set discovery off` |
| Folder for received files, if not Downloads | none | A full folder path. Empty: Downloads, Flow. | `flow set files_folder /home/john/Flow` |

## Commands

    flow run             share or receive the mouse and keyboard, as the settings say
    flow tell            how Flow is getting on, shown in the ring
    flow settings        print the settings
    flow set KEY VALUE   change one setting
    flow forget          unpair and make a new key
    flow allow ID        let a computer asking to pair do so; type the number it shows
    flow use ID          ask a computer Flow found to pair with this one
    flow goto ID         take the pointer over to that computer
    flow switch          go to the other computer, or come back
    flow home            bring the pointer back to this computer
    flow drop ID         disconnect one computer until Connect
    flow connect         share again after a disconnect
    flow disconnect      stop sharing until connect, staying paired
    flow unlock [ID]     unlock that computer's screen with the PIN, read from standard input
    flow version         print the version
    flow help            print this list

## Where your data lives

Flow keeps everything on the two computers, inside its own home in its
Trindle box. Its settings, the computers it is paired with and each one's
key are in `~/.config/flow/config.json`. This computer's certificate and
private key are in `~/.config/flow/cert.pem` and `key.pem`. All three can be
read only by you. Received files go to Downloads, Flow. A damaged settings
file is kept as `config.json.bad` and Flow starts afresh.

## Permissions

Flow asks for the network, the person's folders (received files go to
Downloads, Flow), notifications, running from login, to control the
keyboard, mouse and clipboard (needed on the computer being controlled, and
for the clipboard on both), and to share the mouse and keyboard with
another computer (needed on the controlling computer). Each is off until
allowed in the window that opens when Flow is installed, or later in
Settings, Permissions; a change there takes effect at once. A change to
Trindle itself that adds a permission takes effect after logging out and
back in once.

## Security

The link between the two computers is TLS 1.3 only, and each side pins the
other's certificate. Pairing uses SPAKE2 over the four-digit number, tied to
that link, so the number is never sent and nothing in between can learn it
or test guesses away from the connection: each try allows one guess. After
three wrong tries the number changes, and after three numbers asking stops
until the computer is picked again. Each controlled computer gets a key of
its own, and only private, link-local, loopback and Tailscale addresses are
let in. The PIN is never written down, logged or printed.

Flow runs in its box with no session bus. On the computer being controlled,
Trindle gives the box its own Wayland socket with a made-up mouse (wlr
virtual pointer), a made-up keyboard (virtual keyboard) and the clipboard
(wlr data control). On the controlling computer the same socket also offers
Trindle's input capture (trindle_input_capture_v1, in
protocols/input_capture.xml): Flow names the side the other computer is on,
and when the pointer is pushed off it the desktop hides the pointer and
hands Flow every movement, button, scroll and key until Flow lets go. On the
computer being controlled, Trindle's own Flow alone is also offered
trindle_unlock_v1 (protocols/unlock.xml), which takes the PIN and answers
done, or why not in a few words. No screenshots and no other windows are
offered either way.

To report a security problem, see [SECURITY.md](SECURITY.md).

## Building it yourself

Flow builds with Rust 1.85 or later.

```bash
cargo test
cargo build --release
```

To make the plugin:

```bash
bash packaging/build-trindle-plugin.sh
```

This builds Flow in release, makes the plugin folder org.trindle.Flow with
its plugin.ini, and writes it to the folder given, or ./target/plugin, with
a .tar.zst of it, a description and an icon beside it.

## Licence

Copyright the Trindle project. Flow is free software under the GNU General
Public License, version 3 or (at your option) any later version. The full
text is in [LICENSE](LICENSE). Changes are listed in
[CHANGELOG.md](CHANGELOG.md). Report bugs and ask questions on the
[Buildmorphic community](https://buildmorphic.com);
[GitHub issues](https://github.com/trindle-os/trindle-flow/issues) are also
welcome. See [CONTRIBUTING.md](CONTRIBUTING.md) for what helps.

## Disclaimer

This software is provided as it is, without warranty of any kind.
