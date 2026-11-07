# Flow help

Flow lets you use one computer's mouse and keyboard on another computer
nearby. What you copy on one can be pasted on the other, and copied files
come across too. Both computers need Trindle and Flow.

## Getting started

1. On each computer, open Apps, go to the Plugins page and install Flow.
   Allow what it asks for in the window that opens. You can change this
   later in Settings, Permissions.
2. Flow starts by itself when you log in. If its settings say "Flow is not
   running", allow it to run from login in Settings, Permissions.
3. On the computer whose mouse and keyboard you want to use, open Flow's
   settings and switch on **This computer controls the other one**. Under
   **The other computer is**, choose which side of this screen the other
   computer sits on.
4. On the other computer, leave that switch off. Under **Computer that
   controls this one**, pick the controlling computer. It then shows a
   four-digit number.
5. Back on the controlling computer, pick the computer under **Computer
   asking to be controlled** and type the number under **Number that
   computer shows**.

The number is typed on the controlling computer, not on the one showing it.
After that the two find each other by themselves every time.

## Using it

- Push the pointer off the chosen side of the screen and it carries on on
  the other computer, with the keyboard following. Push it back against the
  side facing home to come back. Running along an edge does not cross.
- **Ctrl+Alt+S** switches between the computers without pushing.
- Hold **Ctrl, Alt and Shift** together for a second to bring the mouse and
  keyboard home, whatever is happening. They also come home when the screen
  locks, when Flow stops, and when the other computer stops answering.

Flow's icon in the ring shows how it is getting on. Its menu has **Bring the
pointer back**, **Connect** and **Disconnect**. Disconnect stops sharing
but keeps the two paired, and Connect starts again. On the controlling
computer the menu also has **Go to**, **Unlock** and **Disconnect** for
each computer that is connected, up to four. On the other computer it lists
the controlling computers found, as **Use** and the name, while it is not
connected.

## Settings

| Setting | Shown on | What it does |
|---|---|---|
| This computer controls the other one | both | On: this computer's mouse and keyboard are shared. Off: this computer is the one controlled. Off at first. |
| Computer that controls this one | controlled | The computer to pair with. Nobody unpairs. |
| Computer asking to be controlled | controlling | The computer to let pair. |
| Number that computer shows | controlling | The four digits the computer asking shows. |
| The other computer is | controlling | Left, Right, Above or Below: the side of this screen the pointer leaves by. Right at first. |
| Cross only by shortcut or menu, not by pushing | controlling | On: pushing against the side never crosses, so you cross with Ctrl+Alt+S or the menu. Off at first. |
| Computer on the left, on the right, above, below | controlling | With more than one other computer, which one sits on each side. |
| Pointer speed on the other computer | controlling | Slower, Normal, Faster or Much faster. Normal at first. |
| Share the clipboard with the other computer | both | Copy on one, paste on the other. On at first. |
| Receive files copied on the other computer | both | Files you copy on the other computer come across. On at first. |
| Keep received files in Downloads, Flow | both | On: received files are kept in the Flow folder in Downloads. Off: they go to a temporary folder that is emptied each time Flow starts. On at first. |
| Address of the other computer, for Tailscale | controlled | The address to use when the other computer is not found on the local network. |
| Look for the other computer on this network | both | Off: Flow neither looks nor answers, so the controlled computer needs the address above. Changing it restarts the link. |
| Folder for received files, if not Downloads | both | A full folder path. Empty keeps them in the Flow folder in Downloads. |

Switching **Share the clipboard** off or on takes effect at once.

## Unlocking the other computer

The controlling computer can unlock the other computer's lock screen with
your PIN. It never uses your password.

1. In Flow's ring menu, choose **Unlock** and the computer's name.
2. Trindle shows its PIN box. Type the four digits and choose Send.

The PIN is the one set for the lock screen on the locked computer. It is
typed in Trindle's own box, goes over Flow's link to the locked computer
and nowhere else, and is never saved, logged or printed. The locked
computer checks it itself. After five wrong tries only the password unlocks
it, and the PIN only works for as long after locking as that computer's
Settings allow.

This only works when both computers have Flow installed from Apps.

## When something goes wrong

**"Flow is not running. Allow it to run from login in Settings,
Permissions."** Do that, then log out and in again.

**"Looking for a computer to control this one."** Neither computer has been
set to control yet. On the computer whose mouse and keyboard you want to
use, switch on **This computer controls the other one**. Both computers
must be on the same network.

**"Found" and a name.** Pick that computer under **Computer that controls
this one** to start pairing.

**"Asking" and a name, with a number.** Type that number on the other
computer, under **Number that computer shows**.

**"The number typed did not match."** Type it again on the controlling
computer. After three wrong tries the number changes, so read it again.
After three numbers Flow stops asking: pick the computer again to start over.

**"That computer is not among those Flow has found."** Check Flow is running
on the other computer and that it is set to control.

**"Looking for" and a name, after the two were paired.** The other computer
is off, asleep, or not running Flow, or its address has changed. Flow looks
for it only on your local network. For a computer it reaches over
Tailscale, type its address on the controlled computer under **Address of
the other computer, for Tailscale**.

**"... is not the computer it was: its identity has changed."** This usually
happens because Flow was reinstalled on the other computer, but it could
also be another computer pretending to be it, so Flow will not use it until
you pair the two again. Choose **Use** and the name in Flow's ring menu, then
type the number it shows on the other computer.

**"Another program is sharing your mouse and keyboard, so Flow is
waiting."** Close the other program and Flow carries on.

**"Flow cannot share your mouse and keyboard"** or **"Flow cannot control
this computer"**. Flow has not been allowed what it needs. In Settings,
Permissions, allow Flow to share your mouse and keyboard on the controlling
computer, and to control your keyboard, mouse and clipboard on the other.

**"Disconnected."** Someone chose Disconnect. Choose **Connect** in Flow's
ring menu.

**The pointer is stuck on the other computer.** Hold Ctrl, Alt and Shift
together for a second.

**What I copied does not arrive.** Check that **Share the clipboard** is on
on both computers. Text and pictures up to 32 MB come across.

**A file does not arrive.** Check that **Receive files copied on the other
computer** is on. Up to 32 files come across from one copy, each up to
100 MB; bigger files are left out. If the link drops part way, the file is
said to be lost, the part that came is removed, and nothing is pasted.

**Unlocking says no.** The locked computer gives the reason:

- "Not locked.": the screen is not locked.
- "No PIN set.": set a PIN for the lock screen on that computer.
- "PIN time is over.": too long has passed since it locked, so use the
  password there.
- "Wrong PIN." with the tries left: type it again.
- "Too many tries.": only the password unlocks it now.
- "This computer's Trindle does not let Flow unlock it.": update Trindle
  and install Flow from Apps.
- "Flow is not running here, or this computer does not control the other
  one.": unlock from the controlling computer, with Flow running.
- "The other computer did not answer in time.": check the two are
  connected and try again.

## Privacy: what Flow can and cannot do

What Flow can do:

- **Your mouse and keys, on the controlling computer:** Flow receives them
  only while the pointer is over on the other computer, and sends them
  there.
- **The other computer:** Flow presses, clicks and types there what you do.
  When Flow is installed from Apps, Trindle counts those presses as your
  own. So they work in password boxes and on power buttons, as your own
  hands would. They never reach a locked screen; only the PIN unlock above
  can open one.
- **The clipboard:** while **Share the clipboard** is on, Flow is told of
  everything you copy and sends it to the other computer. Its log says only
  what kind of thing it was and how big, never what it said.
- **Files:** Flow reads only files you copy, and only while **Receive files
  copied on the other computer** is on.

What Flow cannot do:

- See your screen, or look at or read other windows.
- Talk to anything outside your own network. It has no account and no
  server in between, and it lets in only computers on your local network,
  on Tailscale, or this computer itself.
- Connect to a computer you have not paired. The link is encrypted, and
  each side checks the other is the computer it paired with. The pairing
  number is never sent over the network.

**Where its data is:** Flow keeps its settings, the computers it is paired
with and its keys in its own settings folder, readable only by you. Received
files are kept in Downloads, in a Flow folder.

**One thing to know:** Ctrl+Alt+S, Go to and Bring the pointer back work by
leaving a short note in Flow's settings folder. Any program running as you
could leave that note too, and so move the pointer between your computers,
just as it could by pressing the keys.

Report bugs and ask questions on the [Buildmorphic community](https://buildmorphic.com);
[GitHub issues](https://github.com/trindle-os/trindle-flow/issues) are also welcome.
To report a security problem privately, see [SECURITY.md](../SECURITY.md).
