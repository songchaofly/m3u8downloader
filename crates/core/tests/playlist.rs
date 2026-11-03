use m3u8_core::playlist::parse;

#[test]
fn preserves_media_sequence_and_resolves_against_the_final_playlist_url() {
    let text = "#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXT-X-MEDIA-SEQUENCE:100\n#EXTINF:6,\n../segment.ts?sig=abc%2Fdef\n#EXT-X-ENDLIST\n";
    let list = parse(text, "https://cdn.example/a/b/list?token=secret").unwrap();
    assert_eq!(list.segments[0].sequence, 100);
    assert_eq!(
        list.segments[0].uri,
        "https://cdn.example/a/segment.ts?sig=abc%2Fdef"
    );
    assert_eq!(list.duration(), 6.0);
}

#[test]
fn keeps_key_rotations_ranges_and_discontinuities() {
    let s="#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXT-X-KEY:METHOD=AES-128,URI=\"key\"\n#EXTINF:2,\n#EXT-X-BYTERANGE:188@0\na.ts\n#EXT-X-KEY:METHOD=AES-128,URI=\"key\"\n#EXT-X-DISCONTINUITY\n#EXTINF:2,\n#EXT-X-BYTERANGE:188\na.ts\n#EXT-X-KEY:METHOD=NONE\n#EXTINF:2,\nb.ts\n#EXT-X-ENDLIST";
    let p = parse(s, "https://example.com/list").unwrap();
    assert_eq!(p.segments[1].range.as_ref().unwrap().offset, 188);
    assert_ne!(
        p.segments[0].key.as_ref().unwrap().id,
        p.segments[1].key.as_ref().unwrap().id
    );
    assert_eq!(p.segments[1].discontinuity, 1);
    assert!(p.segments[2].key.is_none());
}
#[test]
fn rejects_unsafe_or_unsupported_playlists() {
    for s in [
        "<html>Login</html>",
        "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\nfile:///private/key\n",
        "#EXTM3U\n#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"key\"",
        "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\n#EXT-X-BYTERANGE:188\na.ts",
    ] {
        assert!(parse(s, "https://example.com/list").is_err());
    }
}
#[test]
fn handles_quoted_codecs_and_audio_groups() {
    let s="#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",NAME=\"English\",URI=\"audio.m3u8\"\n#EXT-X-STREAM-INF:BANDWIDTH=50000,CODECS=\"avc1.42e01e,mp4a.40.2\",AUDIO=\"a\"\nvideo.m3u8";
    let p = parse(s, "https://example.com/list").unwrap();
    assert!(p.master);
    assert_eq!(
        p.variants[0].codecs.as_deref(),
        Some("avc1.42e01e,mp4a.40.2")
    );
    assert_eq!(
        p.renditions[0].uri.as_deref(),
        Some("https://example.com/audio.m3u8")
    );
}
