//! The owner's PUBLIC key for SME Pro. Safe to commit and publish: it can only
//! check licences, never make them. It is empty until the owner runs, on their
//! own computer:
//!
//! ```text
//! python licensing/make_owner_key.py
//! ```
//!
//! That writes the private key file (smp-owner.key, git-ignored: never commit
//! or share it) and fills the key in below, once. While it is empty the app
//! refuses to run (fail closed) and the release workflow refuses to build.
//! Never replace a key that is already set: every issued licence would stop
//! working.
pub const OWNER_PUBLIC_KEY: &str = "de645067d4c692c647ec58f2700f666251a366c1db3a7884b0072da7d02e7f5f";
