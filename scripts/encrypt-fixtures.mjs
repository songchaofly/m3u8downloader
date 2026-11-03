import { createCipheriv } from "node:crypto";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";

const root = process.argv[2];
const keyA = Buffer.from(Array.from({ length: 16 }, (_, i) => i));
const keyB = Buffer.from(Array.from({ length: 16 }, (_, i) => i + 16));
for (const kind of ["encrypted", "explicit", "rotation"]) {
  const folder = path.join(root, kind);
  await mkdir(folder, { recursive: true });
  await writeFile(path.join(folder, "key.bin"), keyA);
  await writeFile(path.join(folder, "key2.bin"), keyB);
  let manifest =
    "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:100\n";
  for (let sequence = 100; sequence < 108; sequence++) {
    if (sequence === 100)
      manifest +=
        kind === "rotation"
          ? '#EXT-X-KEY:METHOD=AES-128,URI="/rotation-key.bin"\n'
          : `#EXT-X-KEY:METHOD=AES-128,URI="key.bin"${kind === "explicit" ? ",IV=0x1234" : ""}\n`;
    if (kind === "rotation" && sequence === 104)
      manifest += '#EXT-X-KEY:METHOD=AES-128,URI="/rotation-key.bin"\n';
    if (kind === "rotation" && sequence === 106)
      manifest += "#EXT-X-KEY:METHOD=NONE\n";
    const plain = await readFile(
      path.join(root, "plain", `segment-${sequence}.ts`),
    );
    const iv = Buffer.alloc(16);
    iv.writeBigUInt64BE(BigInt(kind === "explicit" ? 0x1234 : sequence), 8);
    const cipher = createCipheriv(
      "aes-128-cbc",
      kind === "rotation" && sequence >= 104 ? keyB : keyA,
      iv,
    );
    const data =
      kind === "rotation" && sequence >= 106
        ? plain
        : Buffer.concat([cipher.update(plain), cipher.final()]);
    await writeFile(path.join(folder, `segment-${sequence}.ts`), data);
    manifest += `#EXTINF:1,\nsegment-${sequence}.ts\n`;
  }
  await writeFile(
    path.join(folder, "index.m3u8"),
    manifest + "#EXT-X-ENDLIST\n",
  );
  if (kind === "encrypted")
    await writeFile(
      path.join(folder, "unmarked.m3u8"),
      (manifest + "#EXT-X-ENDLIST\n").replace(/#EXT-X-KEY:[^\n]+\n/g, ""),
    );
}
