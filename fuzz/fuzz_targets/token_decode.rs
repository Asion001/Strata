//! Share tokens and change-feed cursors from the outside: no panic, and
//! anything malformed is refused.
#![no_main]

use libfuzzer_sys::fuzz_target;
use strata_server::journal::Cursor;
use strata_server::shares;

fuzz_target!(|data: &[u8]| {
    let s = String::from_utf8_lossy(data);
    // A token is accepted only in the shape the server makes.
    if shares::token_is_well_formed(&s) {
        assert_eq!(s.len(), 24);
        assert!(
            s.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
    }
    // A cursor that decodes holds no negative id or position, and encodes
    // back to one that decodes to the same.
    if let Ok(c) = Cursor::decode(&s) {
        assert!(c.u >= 0);
        assert!(c.v.iter().all(|(id, seq)| *id >= 0 && *seq >= 0));
        assert_eq!(Cursor::decode(&c.encode()).ok().as_ref(), Some(&c));
    }
    // Hex of the raw bytes reaches the JSON parser too.
    let _ = Cursor::decode(&hex::encode(data));
});
