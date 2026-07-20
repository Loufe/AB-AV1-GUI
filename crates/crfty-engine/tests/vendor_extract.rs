//! Hardened-extraction contract: malicious archive layouts — traversal,
//! absolute paths, links, case collisions, escapes, bombs — reject the whole
//! archive, and only the two manifest binaries ever reach disk.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::io::{Cursor, Write};

use crfty_engine::vendor::{
    extract::{ExtractSpec, extract_binaries},
    manifest::ArchiveKind,
};
use zip::write::SimpleFileOptions;

const FFMPEG_ENTRY: &str = "build/bin/ffmpeg";
const FFPROBE_ENTRY: &str = "build/bin/ffprobe";

fn spec(kind: ArchiveKind) -> ExtractSpec {
    ExtractSpec {
        ffmpeg_entry: FFMPEG_ENTRY.to_owned(),
        ffprobe_entry: FFPROBE_ENTRY.to_owned(),
        max_extracted_bytes: 1024 * 1024,
        kind,
    }
}

/// Appends a tar entry with the name written verbatim into the header,
/// bypassing any sanitizing the builder API performs.
fn raw_tar_entry(
    builder: &mut tar::Builder<Vec<u8>>,
    name: &str,
    entry_type: tar::EntryType,
    contents: &[u8],
) {
    let mut header = tar::Header::new_gnu();
    assert!(name.len() < 100, "raw names must fit the header field");
    header.as_mut_bytes()[..name.len()].copy_from_slice(name.as_bytes());
    header.set_entry_type(entry_type);
    header.set_size(contents.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    builder.append(&header, contents).expect("append tar entry");
}

fn tar_xz(build: impl FnOnce(&mut tar::Builder<Vec<u8>>)) -> tempfile::NamedTempFile {
    let mut builder = tar::Builder::new(Vec::new());
    build(&mut builder);
    let tar_bytes = builder.into_inner().expect("finish tar");
    let mut file = tempfile::NamedTempFile::new().expect("create archive file");
    let mut compressed = Vec::new();
    lzma_rs::xz_compress(&mut Cursor::new(tar_bytes), &mut compressed).expect("compress tar");
    file.write_all(&compressed).expect("write archive");
    file
}

fn expected_tar_layout(builder: &mut tar::Builder<Vec<u8>>) {
    raw_tar_entry(builder, "build/", tar::EntryType::Directory, b"");
    raw_tar_entry(builder, "build/bin/", tar::EntryType::Directory, b"");
    raw_tar_entry(
        builder,
        FFMPEG_ENTRY,
        tar::EntryType::Regular,
        b"ffmpeg binary",
    );
    raw_tar_entry(
        builder,
        FFPROBE_ENTRY,
        tar::EntryType::Regular,
        b"ffprobe binary",
    );
    raw_tar_entry(builder, "build/LICENSE", tar::EntryType::Regular, b"GPL");
}

fn zip_archive(
    build: impl FnOnce(&mut zip::ZipWriter<Cursor<Vec<u8>>>),
) -> tempfile::NamedTempFile {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    build(&mut writer);
    let bytes = writer.finish().expect("finish zip").into_inner();
    let mut file = tempfile::NamedTempFile::new().expect("create archive file");
    file.write_all(&bytes).expect("write archive");
    file
}

fn stored() -> SimpleFileOptions {
    SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored)
}

fn zip_file(writer: &mut zip::ZipWriter<Cursor<Vec<u8>>>, name: &str, contents: &[u8]) {
    writer.start_file(name, stored()).expect("start zip entry");
    writer.write_all(contents).expect("write zip entry");
}

fn extract_error(archive: &tempfile::NamedTempFile, spec: &ExtractSpec) -> String {
    let destination = tempfile::tempdir().expect("create destination");
    extract_binaries(archive.path(), spec, destination.path())
        .expect_err("extraction must be rejected")
}

#[test]
fn extracts_the_two_binaries_from_a_tar_xz() {
    let archive = tar_xz(expected_tar_layout);
    let destination = tempfile::tempdir().expect("create destination");
    let binaries = extract_binaries(
        archive.path(),
        &spec(ArchiveKind::TarXz),
        destination.path(),
    )
    .expect("extraction succeeds");
    assert_eq!(
        std::fs::read(&binaries.ffmpeg).expect("read ffmpeg"),
        b"ffmpeg binary"
    );
    assert_eq!(
        std::fs::read(&binaries.ffprobe).expect("read ffprobe"),
        b"ffprobe binary"
    );
    assert_eq!(
        binaries.ffmpeg,
        destination.path().join("bin").join("ffmpeg")
    );
    // Only the two binaries land on disk — the LICENSE stays in the archive.
    let extracted: Vec<_> = std::fs::read_dir(destination.path().join("bin"))
        .expect("list bin")
        .map(|entry| entry.expect("read entry").file_name())
        .collect();
    assert_eq!(extracted.len(), 2);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&binaries.ffmpeg)
            .expect("stat ffmpeg")
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o755,
            "modes are chosen by the code, not the archive"
        );
    }
}

#[test]
fn extracts_the_two_binaries_from_a_zip() {
    let archive = zip_archive(|writer| {
        zip_file(writer, FFMPEG_ENTRY, b"ffmpeg binary");
        zip_file(writer, FFPROBE_ENTRY, b"ffprobe binary");
        zip_file(writer, "build/LICENSE", b"GPL");
    });
    let destination = tempfile::tempdir().expect("create destination");
    let binaries = extract_binaries(archive.path(), &spec(ArchiveKind::Zip), destination.path())
        .expect("extraction succeeds");
    assert_eq!(
        std::fs::read(&binaries.ffmpeg).expect("read ffmpeg"),
        b"ffmpeg binary"
    );
    assert_eq!(
        std::fs::read(&binaries.ffprobe).expect("read ffprobe"),
        b"ffprobe binary"
    );
}

#[test]
fn tar_path_traversal_rejects_the_archive() {
    let archive = tar_xz(|builder| {
        expected_tar_layout(builder);
        raw_tar_entry(builder, "build/../../evil", tar::EntryType::Regular, b"x");
    });
    let detail = extract_error(&archive, &spec(ArchiveKind::TarXz));
    assert!(detail.contains("traverses"), "{detail}");
}

#[test]
fn tar_absolute_path_rejects_the_archive() {
    let archive = tar_xz(|builder| {
        expected_tar_layout(builder);
        raw_tar_entry(builder, "/etc/evil", tar::EntryType::Regular, b"x");
    });
    let detail = extract_error(&archive, &spec(ArchiveKind::TarXz));
    assert!(
        detail.contains("unusable path") || detail.contains("top-level"),
        "{detail}"
    );
}

#[test]
fn tar_symlink_rejects_the_archive() {
    let archive = tar_xz(|builder| {
        expected_tar_layout(builder);
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        builder
            .append_link(&mut header, "build/bin/evil", "/etc/passwd")
            .expect("append symlink");
    });
    let detail = extract_error(&archive, &spec(ArchiveKind::TarXz));
    assert!(detail.contains("forbidden entry type"), "{detail}");
}

#[test]
fn tar_hardlink_rejects_the_archive() {
    let archive = tar_xz(|builder| {
        expected_tar_layout(builder);
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Link);
        header.set_size(0);
        builder
            .append_link(&mut header, "build/bin/evil", "build/bin/ffmpeg")
            .expect("append hardlink");
    });
    let detail = extract_error(&archive, &spec(ArchiveKind::TarXz));
    assert!(detail.contains("forbidden entry type"), "{detail}");
}

#[test]
fn entry_outside_the_top_level_directory_rejects_the_archive() {
    let archive = tar_xz(|builder| {
        expected_tar_layout(builder);
        raw_tar_entry(builder, "other/stray", tar::EntryType::Regular, b"x");
    });
    let detail = extract_error(&archive, &spec(ArchiveKind::TarXz));
    assert!(detail.contains("top-level"), "{detail}");
}

#[test]
fn zip_case_colliding_duplicates_reject_the_archive() {
    let archive = zip_archive(|writer| {
        zip_file(writer, FFMPEG_ENTRY, b"ffmpeg binary");
        zip_file(writer, FFPROBE_ENTRY, b"ffprobe binary");
        zip_file(writer, "build/bin/FFmpeg", b"impostor");
    });
    let detail = extract_error(&archive, &spec(ArchiveKind::Zip));
    assert!(detail.contains("case-colliding"), "{detail}");
}

#[test]
fn zip_symlink_rejects_the_archive() {
    let archive = zip_archive(|writer| {
        zip_file(writer, FFMPEG_ENTRY, b"ffmpeg binary");
        zip_file(writer, FFPROBE_ENTRY, b"ffprobe binary");
        writer
            .add_symlink("build/bin/evil", "/etc/passwd", stored())
            .expect("append symlink");
    });
    let detail = extract_error(&archive, &spec(ArchiveKind::Zip));
    assert!(detail.contains("symlink"), "{detail}");
}

#[test]
fn zip_backslash_name_rejects_the_archive() {
    let archive = zip_archive(|writer| {
        zip_file(writer, FFMPEG_ENTRY, b"ffmpeg binary");
        zip_file(writer, FFPROBE_ENTRY, b"ffprobe binary");
        zip_file(writer, "build\\bin\\evil", b"x");
    });
    let detail = extract_error(&archive, &spec(ArchiveKind::Zip));
    assert!(detail.contains("backslash"), "{detail}");
}

#[test]
fn zip_path_traversal_rejects_the_archive() {
    let archive = zip_archive(|writer| {
        zip_file(writer, FFMPEG_ENTRY, b"ffmpeg binary");
        zip_file(writer, FFPROBE_ENTRY, b"ffprobe binary");
        zip_file(writer, "build/../evil", b"x");
    });
    let detail = extract_error(&archive, &spec(ArchiveKind::Zip));
    assert!(detail.contains("traverses"), "{detail}");
}

#[test]
fn archive_missing_a_binary_is_rejected() {
    let archive = tar_xz(|builder| {
        raw_tar_entry(
            builder,
            FFMPEG_ENTRY,
            tar::EntryType::Regular,
            b"ffmpeg binary",
        );
    });
    let detail = extract_error(&archive, &spec(ArchiveKind::TarXz));
    assert!(detail.contains("does not contain"), "{detail}");
}

#[test]
fn tar_expanding_past_the_cap_is_rejected() {
    let archive = tar_xz(|builder| {
        raw_tar_entry(
            builder,
            FFMPEG_ENTRY,
            tar::EntryType::Regular,
            &[0_u8; 8 * 1024],
        );
        raw_tar_entry(builder, FFPROBE_ENTRY, tar::EntryType::Regular, b"ffprobe");
    });
    let mut small = spec(ArchiveKind::TarXz);
    small.max_extracted_bytes = 4 * 1024;
    let detail = extract_error(&archive, &small);
    assert!(detail.contains("size limit"), "{detail}");
}

#[test]
fn zip_expanding_past_the_cap_is_rejected() {
    let archive = zip_archive(|writer| {
        zip_file(writer, FFMPEG_ENTRY, &[0_u8; 8 * 1024]);
        zip_file(writer, FFPROBE_ENTRY, b"ffprobe binary");
    });
    let mut small = spec(ArchiveKind::Zip);
    small.max_extracted_bytes = 4 * 1024;
    let detail = extract_error(&archive, &small);
    assert!(detail.contains("size limit"), "{detail}");
}
