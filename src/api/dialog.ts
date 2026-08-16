import { open } from "@tauri-apps/plugin-dialog";

export async function chooseLocalAudioFile(): Promise<string | null> {
  const selected = await open({
    multiple: false,
    directory: false,
    filters: [{ name: "Audio", extensions: ["wav", "mp3", "flac", "ogg"] }],
  });

  return typeof selected === "string" ? selected : null;
}

export async function chooseMarkdownDirectory(): Promise<string | null> {
  const selected = await open({ multiple: false, directory: true });

  return typeof selected === "string" ? selected : null;
}
