# Security policy

Flow is a small project kept up in the time available, not a team with a dedicated security function. This page sets realistic expectations.

## Supported versions

Only the latest release is supported. There is no long-term-support branch and no backporting. A fix lands in the next release and you are expected to update.

| Version | Supported |
|---|---|
| 2.6.x | yes |
| older | no |

## Reporting a vulnerability

**Please do not put the details of a security problem in a public issue.** A public report tells everyone, including people running an unpatched copy, before a fix exists.

Use GitHub's private vulnerability reporting on this repository, under **Security**, then **Report a vulnerability**. Only the maintainers can see it.

If you would rather not use GitHub, write to <distro@trindle.org> with "Flow security" in the subject line and say only that you have a security issue. Leave the details out of that first message and a private channel will be arranged.

## What to expect

Replies and fixes are best effort, not guaranteed against any service level. You will be told when the report is read, and when a fix is released.

## What is in scope

Flow treats the network as hostile. In scope: anything that lets a computer other than the paired one connect, pair, send input or files, or read the clipboard; a way to learn or test the pairing number away from the connection; a received file landing outside its folder; the PIN being kept, logged or sent anywhere but the locked computer; and anything that lets one computer hang or crash the other. Faults in Trindle itself, or in a computer that is already in someone else's hands, are not in scope here.

## Known and accepted

- On the controlling computer Flow listens on every network address and then refuses any connection that does not come from a private, link-local, loopback or Tailscale address. The port is reachable until that check, which happens before anything is read.
- `flow switch`, `flow goto` and `flow home` leave a short note in Flow's own settings folder, and Flow acts on it only if it is under five seconds old. Any program running as you can leave that note, so it can move the pointer between your computers, as it could by pressing the keys.
