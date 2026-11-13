# Changelog

## 2.6.5 (13 November 2026)

- Everything Flow says to you is rewritten in plain, everyday English: its messages, its settings, the command-line help, this README and the help page. Nothing Flow does has changed.
- Four settings have shorter names: Cross only by shortcut or menu, Tailscale address of the other computer, Keep received files in Downloads/Flow, and Different folder for received files.

## 2.6.4 (11 November 2026)

- Another machine on the network answering with the paired computer's name can no longer keep the two apart: each answer is kept by who sent it, and a stranger's certificate makes Flow try the typed address and the last one used.
- A computer that is not paired is written to the log, and announced, at most three times a minute.
- No more than 4 GB of copied files comes in within an hour, so the other computer cannot fill the disk.
- While the other computer is off or unpaired, the controlled computer logs it once instead of every three seconds, and no longer counts events.
- The plugin now carries the licences of the code built into it, in share/licences.
- Less work while idle: the screens are looked up at most every ten seconds, the settings are read again only when they change, and looking for the other computer sleeps between questions.
- A copy that loses its turn to a newer one part way is dropped without a word.
- Flow starting at the same moment as `flow tell` no longer stops with "already running".

## 2.6.3 (5 November 2026)

- Three settings that could only be changed by command are now on the settings page: the address of the other computer, looking for it on the local network, and the folder for received files. Switching the looking on or off restarts the link.
- The address is now a setting of its own, `address`, kept apart from the last address used, so finding the other computer nearby no longer overwrites a typed Tailscale address. `flow set server_host` is gone; use `flow set address`.
- When Flow cannot save its settings, on a full disk for example, it now says so once, in the log and as a notification. Before, six of its saves failed without a word and a pairing or a setting could be lost.
- Switching Share the clipboard off now stops Flow reading the clipboard at once. Before, it only stopped sending, and kept reading what was copied until Flow restarted.
- With Cross only by shortcut or menu on, changing the side the other computer is on no longer lets a push cross.
- With three or more computers, files copied on two of them at once no longer get mixed up: the controlling computer passes on one copy at a time, the newest.
- A pairing number that cannot be marked as used is now refused, so a full disk cannot let one number be used twice.

## 2.6.2 (3 November 2026)

- Trust again is gone from the ring menu, along with the `flow trust` command. It let a computer pretending to be the paired one take over with one press. When the other computer's identity changes, the two are paired again with the four-digit number.
- The warning that the other computer's identity has changed now stays in Flow's status and is sent once as a notification. Before, it only reached the log.
- Switching Share the clipboard back on starts it at once, instead of when Flow next starts.

## 2.6.1 (1 November 2026)

- The pointer crosses to the real edge of a real screen of the other computer. Each computer now tells the other where its screens are, in points, so a layout with an empty margin (screens starting at x 1290, say), scaled screens, or stacked screens no longer adds travel that is not there, going over or coming back. Both computers need this version.

## 2.6.0 (26 October 2026)

- Pairing needs the four-digit number the asking computer shows, typed on the controlling one. The two check it with SPAKE2 over their link, so it is never sent. Three wrong tries change the number; after three numbers asking stops.
- The link is TLS 1.3 only. Connections are taken only from private, link-local, loopback and Tailscale addresses, and a computer that is slow to say hello is cut off.
- Each controlled computer gets a key of its own, so one disconnected from the menu cannot come back under another name.
- The controlled computer lets go of a controlling one that has said nothing for 15 seconds.
- A file that stops part way is said to be lost and its half removed, and a copy with a lost file is never pasted.
- Disconnect, Connect or swapping round no longer leaves a second keyboard, pointer or clipboard watcher behind.
- Only the latest eight computers asking to pair are listed.
- Change to how a command works: `flow allow` now takes only the ID, and the pairing number is typed on standard input instead of after the ID, so it never shows on the command line. `flow allow ID NUMBER` no longer works.
- Answers to computers looking for Flow are limited per second, and looking for the other computer covers only the local network and slows down when nothing is found.
- The port can no longer be changed from the settings, as the plugin always listens on 24810.
- A monitor plugged in on the controlled computer is followed without reconnecting.
- A damaged settings file is kept as config.json.bad instead of being lost.
- Switching off Share the clipboard stops sending what you copy at once, instead of when Flow next starts.
- When idle, Flow wakes about twice a second instead of 20 to 30 times, waiting for things to happen instead of looking.
- Hashing and randomness come from ring, certificates from rcgen 0.14, and the other libraries are at their latest compatible versions. base64 stays at 0.22 while rcgen's pem uses it, and wayland-protocols at 0.32.12 while Flow builds with Rust 1.85.

## 2.5.0 (19 October 2026)

- Unlock in the ring menu, for each connected computer, unlocks its lock screen with your PIN through Trindle's PIN box.

## 2.4.6 (12 October 2026)

- Something copied a second time, or still copied when Flow started, is sent again instead of being quietly left behind.

## 2.4.5 (12 October 2026)

- A copied picture is sent whole: anything over 64 KB was cut short on the way, which broke the connection.

## 2.4.4 (12 October 2026)

- Pictures up to 32 MB are carried, and the log says what kind of thing was copied, sent and arrived, never the thing itself.

## 2.4.3 (10 October 2026)

- Flow stops when its way to the desktop is lost for good, so Trindle starts it afresh.

## 2.4.2 (10 October 2026)

- The log says when the desktop hands Flow a key while the pointer is home.

## 2.4.1 (10 October 2026)

- The top of the settings says how Flow is getting on.

## 2.4.0 (10 October 2026)

- No pairing code: the settings list the computers found and those asking, to pick from. Which computer controls is one switch.

## 2.3.1 (10 October 2026)

- Only the settings that apply to this computer's role are shown.

## 2.3.0 (10 October 2026)

- Files you copy from any folder are carried across.

## 2.2.0 (7 October 2026)

- Disconnect and Connect from the ring menu, without unpairing.

## 2.1.0 (6 October 2026)

- This computer's own certificate, pairing, and answering when asked where it is.
