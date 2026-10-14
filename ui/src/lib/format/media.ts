import type { AudioCodec, MediaContainer, VideoCodec } from "@/lib/bindings";

function otherLabel(value: { Other: string }): string {
  return value.Other.length > 0 ? value.Other.toUpperCase() : "OTHER";
}

export function videoCodecLabel(codec: VideoCodec | null): string {
  if (codec === null) return "—";
  if (typeof codec === "object") return otherLabel(codec);
  const labels: Record<Exclude<VideoCodec, object>, string> = {
    Av1: "AV1",
    H264: "H.264",
    Hevc: "HEVC",
    Vp9: "VP9",
  };
  return labels[codec];
}

export function containerLabel(container: MediaContainer | null): string {
  if (container === null) return "—";
  if (typeof container === "object") return otherLabel(container);
  return "MKV";
}

function audioCodecLabel(codec: AudioCodec): string {
  if (typeof codec === "object") return otherLabel(codec);
  const labels: Record<Exclude<AudioCodec, object>, string> = {
    Aac: "AAC",
    Ac3: "AC3",
    Eac3: "E-AC3",
    Dts: "DTS",
    Opus: "Opus",
    Flac: "FLAC",
    Mp3: "MP3",
  };
  return labels[codec];
}

export function audioSummary(audio: AudioCodec[] | null): string {
  if (audio === null) return "—";
  if (audio.length === 0) return "No audio";
  if (audio.length > 3) return `${audio.length} audio`;
  return audio.map(audioCodecLabel).join(", ");
}
