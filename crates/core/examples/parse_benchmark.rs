use std::time::Instant;
fn main() {
    let mut source = "#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:100\n".to_string();
    for i in 0..1000 {
        source.push_str(&format!("#EXTINF:2,\nsegment-{i}.ts\n"));
    }
    source.push_str("#EXT-X-ENDLIST\n");
    let mut timings = Vec::new();
    for sample in 0..105 {
        let start = Instant::now();
        let parsed =
            m3u8_core::playlist::parse(&source, "https://fixture.invalid/media/index.m3u8")
                .unwrap();
        assert_eq!(parsed.segments.len(), 1000);
        if sample >= 5 {
            timings.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        std::hint::black_box(parsed);
    }
    let mut sorted = timings.clone();
    sorted.sort_by(f64::total_cmp);
    println!(
        "{}",
        serde_json::json!({"segments":1000,"warmup":5,"samples_ms":timings,"p95_ms":sorted[94],"threshold_ms":200,"passed":sorted[94]<200.0})
    );
}
