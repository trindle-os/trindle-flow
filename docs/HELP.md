# Flow help

Flow lets you use the mouse and keyboard of one computer on another
computer nearby. What you copy on one can be pasted on the other, and copied
files come across too. Both computers need Trindle and Flow.

## Getting started

1. On each computer, open Apps, go to the Plugins page and install Flow.
   Allow what it asks for in the window that opens. You can change this
   later in Settings, Permissions.
2. Flow starts by itself when you log in. If its settings say "Flow is not
   running", go to Settings, Permissions and allow it to run from login.
3. On the computer whose mouse and keyboard you want to use, open Flow's
   settings and switch on **This computer controls the other one**. Under
   **The other computer is**, choose which side of this screen the other
   computer sits on.
4. On the other computer, leave that switch off. Under **Computer that
   controls this one**, pick the controlling computer. The other computer
   then shows a four-digit number.
5. Go back to the controlling computer. Under **Computer asking to be
   controlled**, pick the other computer. Then type the number under
   **Number that computer shows**.

You type the number on the controlling computer, not on the one showing it.
After that, the two computers find each other by themselves every time.

## Using it

- Push the pointer off the side of the screen you chose. It carries on on
  the other computer, and the keyboard goes with it. To come back, push the
  pointer against the side that faces home. Running along an edge doesn't
  cross.
- Press **Ctrl+Alt+S** to switch between the computers without pushing.
- Hold **Ctrl, Alt and Shift** together for a second to bring the mouse and
  keyboard home, whatever is happening. They also come home when the screen
  locks, when Flow stops, and when the other computer stops answering.

Flow's icon in the ring shows how it is getting on. Its menu has **Bring the
pointer back**, **Connect** and **Disconnect**. Disconnect stops sharing but
the two computers stay paired. Connect starts sharing again.

On the controlling computer, the menu also has **Go to**, **Unlock** and
**Disconnect** for each connected computer, up to four. On the other
computer, while it isn't connected, the menu lists the controlling computers
Flow found. Each one appears as **Use** and the name.

## Settings

| Setting | Shown on | What it does |
|---|---|---|
| This computer controls the other one | both | On: this computer's mouse and keyboard are shared. Off: this computer is the one being controlled. Off at first. |
| Computer that controls this one | controlled | The computer to pair with. Pick Nobody to unpair. |
| Computer asking to be controlled | controlling | The computer you allow to pair. |
| Number that computer shows | controlling | The four digits the other computer shows. |
| The other computer is | controlling | Left, Right, Above or Below: the side of this screen the pointer leaves from. Right at first. |
| Cross only by shortcut or menu | controlling | On: pushing against the side doesn't cross, so you cross with Ctrl+Alt+S or the menu. Off at first. |
| Computer on the left, on the right, above, below | controlling | If you have more than one other computer, which one sits on each side. |
| Pointer speed on the other computer | controlling | Slower, Normal, Faster or Much faster. Normal at first. |
| Share the clipboard with the other computer | both | Copy on one, paste on the other. On at first. |
| Receive files copied on the other computer | both | Files you copy on the other computer come across. On at first. |
| Keep received files in Downloads/Flow | both | On: received files are kept in the Flow folder in Downloads. Off: they go to a temporary folder, which Flow empties each time it starts. On at first. |
| Tailscale address of the other computer | controlled | The address to use when Flow can't find the other computer on the local network. |
| Look for the other computer on this network | both | Off: Flow doesn't look for the other computer or answer it, so the controlled computer needs the address above. Changing this restarts the link. |
| Different folder for received files | both | A full folder path. If you leave it empty, received files stay in the Flow folder in Downloads. |

Switching **Share the clipboard** on or off takes effect at once.

## Unlocking the other computer

The controlling computer can unlock the other computer's lock screen with
your PIN. It never uses your password.

1. In Flow's ring menu, choose **Unlock** and the computer's name.
2. Trindle shows its PIN box. Type the four digits and choose Send.

The PIN is the one you set for the lock screen on the locked computer. You
type it in Trindle's own box. It goes over Flow's link to the locked
computer and nowhere else. Flow never saves, logs or prints it.

The locked computer checks the PIN itself. After five wrong tries, only the
password unlocks it. The PIN only works for as long after locking as that
computer's Settings allow.

This only works when both computers have Flow installed from Apps.

## When something goes wrong

**"Flow is not running. In Settings, Permissions, allow it to run when
you log in."** Do that, then log out and in again.

**"Looking for a computer to control this one."** Neither computer is set to
control yet. On the computer whose mouse and keyboard you want to use,
switch on **This computer controls the other one**. Both computers must be
on the same network.

**"Found" and a name.** Pick that computer under **Computer that controls
this one** to start pairing.

**"Asking" and a name, with a number.** Type that number on the other
computer, under **Number that computer shows**.

**"The number you typed ... did not match."** Type it again on the controlling
computer. After three wrong tries the number changes, so read it again.
After three numbers, Flow stops asking. Pick the computer again to start
over.

**"Flow has not found that computer."** Check that Flow is
running on the other computer and that it is set to control.

**"Looking for" and a name, after the two were paired.** The other computer
is off, asleep or not running Flow, or its address has changed. Flow looks
for it only on your local network. If you reach the computer over Tailscale,
type its address on the controlled computer under **Tailscale address of the other
computer**.

**"... is not the computer it was."** Its identity has changed. This usually
happens because you reinstalled Flow on the other computer. It could also be
another computer pretending to be it. So Flow won't use it until you pair
the two again. Choose **Use** and the name in Flow's ring menu, then type
the number it shows on the other computer.

**"Another program is sharing your mouse and keyboard, so Flow is
waiting."** Close the other program, and Flow carries on.

**"Flow cannot share your mouse and keyboard"** or **"Flow cannot control
this computer"**. You haven't allowed Flow what it needs. Go to Settings,
Permissions. On the controlling computer, allow Flow to share your mouse and
keyboard. On the other computer, allow it to control your keyboard, mouse
and clipboard.

**"Disconnected."** Someone chose Disconnect. To share again, choose
**Connect** in Flow's ring menu.

**The pointer is stuck on the other computer.** Hold Ctrl, Alt and Shift
together for a second.

**What I copied does not arrive.** Check that **Share the clipboard** is
switched on for both computers. Text and pictures up to 32 MB come across.

**A file does not arrive.** Check that **Receive files copied on the other
computer** is switched on. Up to 32 files come across from one copy, and each
can be up to 100 MB. Flow leaves out bigger files. No more than 4 GB comes
across in an hour. If the link drops part way, Flow counts the file as lost
and removes the part that arrived. Nothing is pasted.

**Unlocking says no.** The locked computer tells you the reason:

- "Not locked.": the screen isn't locked.
- "No PIN set.": set a PIN for the lock screen on that computer.
- "PIN time is over.": too long has passed since it locked, so use the
  password there.
- "Wrong PIN." with the tries left: type it again.
- "Too many tries.": only the password unlocks it now.
- "Flow cannot unlock this computer with this version of Trindle.": update Trindle
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
- **The other computer:** Flow presses, clicks and types there whatever you
  do. When you install Flow from Apps, Trindle counts those presses as your
  own. So they work in password boxes and on power buttons, just as your own
  hands would. They never reach a locked screen. Only the PIN unlock above
  can open one.
- **The clipboard:** while **Share the clipboard** is on, Flow learns of
  everything you copy and sends it to the other computer. Its log records
  only what kind of thing it was and how big, never the contents.
- **Files:** Flow reads only the files you copy, and only while **Receive
  files copied on the other computer** is on.

What Flow cannot do:

- See your screen, or look at or read other windows.
- Talk to anything outside your own network. It has no account and no
  server in between. It lets in only computers on your local network, on
  Tailscale, or this computer itself.
- Connect to a computer you haven't paired. The link is encrypted, and each
  side checks that the other is the computer it paired with. The pairing
  number is never sent over the network.

**Where its data is:** Flow keeps its settings, the computers it is paired
with and its keys in its own settings folder, and only you can read it.
Received files are kept in Downloads, in a Flow folder.

**One thing to know:** Ctrl+Alt+S, Go to and Bring the pointer back work by
leaving a short note in Flow's settings folder. Any program running as you
could leave that note too. It could then move the pointer between your
computers, just as it could by pressing the keys.

You can report bugs and ask questions on the
[Buildmorphic community](https://buildmorphic.com). You can also use
[GitHub issues](https://github.com/trindle-os/trindle-flow/issues). To report
a security problem privately, see [SECURITY.md](../SECURITY.md).
