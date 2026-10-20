# SME Pro licensing: owner routine

One licence per computer, checked offline (Ed25519). The app only verifies; only
you can make licences. App id `SME Pro`, prefix `SMP` (baked into every licence:
never change them).

## One-time setup (on YOUR computer, never in CI or a shared/AI session)
```
python licensing/make_owner_key.py
```
- writes `smp-owner.key` (the PRIVATE key; already git-ignored). Back it up in two
  places (USB + password manager). Never commit, email or paste it anywhere.
- puts the PUBLIC key into `src-tauri/src/owner_key.rs` (only if still empty).
  Commit that file. Never replace a key once licences have been issued.

Until the public key is set the app refuses to run and the release workflow
refuses to build (fail closed).

## Giving a customer access
1. They open SME Pro, read the **activation code** on screen, send it to you.
2. `python licensing/issue_licence.py --code ABCD-EFGH-JKLM --user "Customer Name" --days 30 --log ~/licences-issued.csv`
3. Send back only the licence text (`SMP1.…`). They paste it in and press Activate.
4. Renewal: same command, same name, new days; they use **Renew** (shown 14 days before expiry).
5. "It doesn't work": `python licensing/verify_licence.py --token <paste> --code <their code>`

## Issuing from any other computer
`python licensing/make_kit.py` makes a folder `smepro-licence-kit` (five tool files, `licensing.json` with the public
key, `issue-licence.bat`). Copy it and your private `smp-owner.key` to any computer with Python 3, put the key in the
same folder, and run `issue-licence.bat` (Windows) or `python issue_licence.py --code ... --user ... --days ...`.
Nothing needs editing and the project isn't needed. The kit holds no secret. Alternatively, claim the owner copy of the
app itself on that computer (below) and use the Licence Manager.

## Your own computer (owner copy, never expires)
On the activation screen open **I am the owner of this app**, paste the contents of
`smp-owner.key`. It is used once and never stored.

## Limits (be honest with customers and yourself)
No revocation (use 30-90 day licences). The check runs on the customer's machine, so
it stops casual copying and sharing, not a determined reverse-engineer. Copying the
app data folder to another PC copies its identity. Wind-back of the clock is refused.
Losing the owner key means no new licences, ever.

Tests: `python licensing/selftest.py` (kit) and `cargo test --lib licensing` (Rust).

## Licence Manager inside the app (owner only, no commands)
On your own computer (the claimed owner copy) a small **Licences** tab appears on the right edge of the window.
1. Open it and **Unlock** with your owner key (choose `smp-owner.key` or paste it). The key is held in memory only,
   never saved, and the manager locks again after 30 idle minutes or when you close the app.
2. Type the customer's activation code and name, press **Generate licence**, copy it and send it.
3. The list shows every customer, when their licence ends and whether it is active, expiring or expired.
   Use the month arrows and tick **Paid** for a month: a fresh licence is made, valid to the end of the latest
   paid month plus 5 days. Send it; they paste it into Renew.
4. **Export (Excel / CSV)** saves the list. The list lives only on this computer (file `licence_customers.json` in
   the app data folder, not in business backups): export from time to time.
Customers' copies never show this tab, and the app refuses every manager request unless it is the owner copy AND unlocked.
The command-line tools above keep working the same way.

## Same name, second computer (Licence Manager)
Adding a code whose customer name already has a live computer never goes through silently. The manager asks
*"<name> already has a computer, ending <date>. Is this a migration or an extra computer?"*
- **Migration**: the old code is marked **retired** in the list and is never renewed or ticked again. Its last
  licence still works until its own date (licences can't be revoked), so use short licences.
- **Extra computer**: recorded as a second seat, listed as its own row and charged like any other.
The list shows how many live computers each customer has (also in the CSV export, column "Computers").
Names are compared ignoring capitals and extra spaces. The command-line tools do not use this list, so they
are not checked.
