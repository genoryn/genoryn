# Recovery phrase and wallet file (GENORYN 1.2)

Two things give access to a GENORYN wallet's coins: its **24-word recovery phrase** and its
**wallet file together with the password**. GENORYN 1.2 makes the phrase available again only
to someone who knows the password, keeps it off the screen otherwise, and makes a stolen file
much more expensive to attack.

## Showing the recovery phrase

Settings → **Wallet security** → *Show recovery phrase*:

1. The wallet asks for its password **again**. The password the open session holds is never
   used for this; the typed one goes to the wallet CLI in its standard input for one
   `reveal_phrase` request, which decrypts the phrase from the file and answers only if the
   password is right.
2. The words appear as a numbered grid, **masked** until *Show words* is pressed. They are drawn
   as non-selectable text: there is no copy button and nothing to select, so they never pass
   through the clipboard.
3. They are wiped from the wallet's memory and the screen after **60 seconds**, when the window
   loses focus or is minimized, on *Done*, on close, on lock and on the 5-minute inactivity
   lock. Showing them again asks for the password again.
4. After **3 wrong passwords** each attempt waits: 30 s, then 60 s, doubling up to 10 minutes.
   Every attempt also costs the full key derivation (about half a second).
5. *Check my backup* hides the words and asks for three random positions from your paper. A
   match records the backup confirmation shown on the Privacy & Security page.

The phrase shown right after creating a wallet uses the same grid, and the Restore form masks
the words being typed.

## Kept out of screen captures

While recovery words are on screen — a new wallet's, revealed ones, or ones being typed into
Restore — the wallet asks the system to keep its window out of captures:

| System | Effect |
|---|---|
| Windows 10 version 2004 and later | Screenshots, screen recording, screen sharing and remote-desktop capture leave the window out (`WDA_EXCLUDEFROMCAPTURE`). |
| Earlier Windows | Captures show the window as a black area (`WDA_MONITOR`). |
| Linux (X11, Wayland) | No such switch exists for applications. The wallet says so in the window and asks you to make sure no recording, sharing or remote session is running. |

The window states which of these applied. The single system call lives in its own small crate,
`genoryn-screen-guard`; the wallet application itself still forbids unsafe code.

## The wallet file

A wallet file is a small JSON envelope: a random salt and nonce, and the 32-byte seed entropy
plus account number encrypted with **XChaCha20-Poly1305** under a key derived from the password
with **Argon2id**. The recovery words are never stored in it.

| | Version 1 (to 1.1) | Version 2 (1.2 and later) |
|---|---|---|
| Argon2id memory | 64 MiB | **256 MiB** |
| Passes / lanes | 3 / 1 | 3 / 1 |
| Cost parameters | implied | stored in the file and **authenticated** with it |

Whoever has the file but not the password can only guess passwords, paying 256 MiB of memory
and three passes for every guess. Lowering the parameters in the file does not help: they are
part of the authenticated header, so decryption fails. A file asking for absurd parameters is
refused before any work is done. Opening a file takes about half a second on a current PC.

GENORYN 1.2 opens version 1 files exactly as before and never rewrites a file by itself.

### Change the password or strengthen the file

Settings → Wallet security → *Change password* (*Strengthen file / change password* for a
version 1 file). Enter the current password and a new one, or the same one twice to keep it and
only strengthen the file. The wallet then:

1. opens the file with the current password (a wrong one changes nothing);
2. writes a new version 2 file **beside** the old one;
3. opens the new file with the new password and checks it holds the same seed, account and
   network;
4. only then moves it over the old file. If any step fails, the old file stays as it was and the
   temporary file is removed.

Copies of the old file elsewhere (backups, other folders) keep the old password and the old
cost. Replace or delete them if the old password may be known. A file system may keep old data
blocks on disk; full-disk encryption protects against that.

## Passwords

The password is the only thing protecting a copied wallet file. New and changed passwords need
at least **12 characters** and a strength estimate of at least 50 bits: the character classes
used, with repeats and runs (`aaaa`, `abcd`, `1234`) and very common words (`password`,
`qwerty`, `genoryn`, …) counting for little. A meter shows the estimate as you type. Several
unrelated words, or words with digits and symbols, are a good choice. Existing passwords keep
working.

## Command line

```
genoryn-wallet-cli --wallet my.gnr protection        # how the file is protected (no password)
genoryn-wallet-cli --wallet my.gnr recovery-phrase   # asks for the password, prints the 24 words
genoryn-wallet-cli --wallet my.gnr change-password   # asks for the current and the new password
```

JSON over standard input (`--json-stdio`): `wallet_protection`, `reveal_phrase` (needs
`password`) and `change_password` (needs `password` and `new_password`).

## What this does not protect against

- **Malware already running as you** can read the screen by other means, log keystrokes (and so
  the password), or read the process's memory. Keep the computer clean and up to date.
- **A camera or a person** looking at the screen.
- **A weak password** on a stolen file: the cost per guess is high, but a guessable password is
  still guessed.
- **The paper copy** of the 24 words: anyone who finds it controls the wallet. Keep it offline
  and private.

## Evidence

- `crates/genoryn-wallet-core/src/keys.rs` tests: version 1 files open and upgrade only through a
  checked password change; a wrong current password leaves the file byte for byte unchanged; no
  temporary file remains; lowered, absurd or missing cost parameters are refused.
- `apps/genoryn-wallet/src/recovery.rs` tests: strength estimate rejects patterns and common
  words and accepts passphrases (including Georgian script); wrong passwords wait longer up to
  the limit; closing wipes the phrase and every typed password.
- `scripts/test/devnet.py` check `recovery_phrase_only_with_password_and_checked_password_change`
  on a real devnet: the phrase only for the right password (wrong and empty refused), version 2
  at 256 MiB, password change refused without the old password, old password refused afterwards,
  same balance and phrase with the new one.
