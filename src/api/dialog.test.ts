import { beforeEach, describe, expect, it, vi } from "vitest";

const openMock = vi.hoisted(() => vi.fn());

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: openMock }));

const dialogModulePath = "./dialog";

describe("dialog pickers", () => {
  beforeEach(() => {
    openMock.mockReset();
  });

  it("requests one local audio file and preserves cancellation", async () => {
    openMock.mockResolvedValueOnce(null);
    const { chooseLocalAudioFile } = await import(/* @vite-ignore */ dialogModulePath);

    await expect(chooseLocalAudioFile()).resolves.toBeNull();
    expect(openMock).toHaveBeenCalledOnce();
    expect(openMock).toHaveBeenCalledWith({
      multiple: false,
      directory: false,
      filters: [{ name: "Audio", extensions: ["wav", "mp3", "flac", "ogg"] }],
    });
  });

  it("requests one Markdown directory and preserves cancellation", async () => {
    openMock.mockResolvedValueOnce(null);
    const { chooseMarkdownDirectory } = await import(/* @vite-ignore */ dialogModulePath);

    await expect(chooseMarkdownDirectory()).resolves.toBeNull();
    expect(openMock).toHaveBeenCalledOnce();
    expect(openMock).toHaveBeenCalledWith({ multiple: false, directory: true });
  });
});
