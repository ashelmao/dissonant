//! The note-recorder fixture, built as a real `.clap` shared object.
//!
//! See `note_recorder_plugin.rs` for the plugin itself and why it exists.

mod recorder {
    // The test reads the log back through `decode`, which this build never calls.
    #![allow(dead_code)]

    include!("note_recorder_plugin.rs");
}

clack_plugin::clack_export_entry!(recorder::NoteRecorderEntry);
