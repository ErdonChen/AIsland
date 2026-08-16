import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { ClipboardItem, CommandError } from "../../api/contracts";
import { I18nProvider } from "../../i18n/I18nProvider";
import ClipboardPage from "./ClipboardPage";

const mocks = vi.hoisted(() => ({
  beginClipboardItemsSubscription: vi.fn(),
  clearClipboardHistory: vi.fn(),
  confirm: vi.fn(),
  copyClipboardItem: vi.fn(),
  createObjectURL: vi.fn(),
  deleteClipboardItem: vi.fn(),
  dispose: vi.fn(),
  getClipboardAsset: vi.fn(),
  invoke: vi.fn(),
  retry: vi.fn(),
  revokeObjectURL: vi.fn(),
  setClipboardPinned: vi.fn(),
  subscribeClipboardItems: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("../../api/commands", () => ({
  clearClipboardHistory: mocks.clearClipboardHistory,
  copyClipboardItem: mocks.copyClipboardItem,
  deleteClipboardItem: mocks.deleteClipboardItem,
  getClipboardAsset: mocks.getClipboardAsset,
  setClipboardPinned: mocks.setClipboardPinned,
}));
vi.mock("../../api/events", () => ({
  beginClipboardItemsSubscription: mocks.beginClipboardItemsSubscription,
  subscribeClipboardItems: mocks.subscribeClipboardItems,
}));

const itemFixture = (overrides: Partial<ClipboardItem> = {}): ClipboardItem => ({
  id: "item-1",
  contentKind: "text",
  textContent: "Build output",
  assetId: null,
  sourceApp: "terminal.exe",
  pinned: false,
  capturedAt: Date.UTC(2026, 7, 8, 10, 0),
  lastSeenAt: Date.UTC(2026, 7, 8, 10, 5),
  byteSize: 12,
  ...overrides,
});
const imageFixture = (overrides: Partial<ClipboardItem> = {}): ClipboardItem => itemFixture({
  id: "image-1",
  contentKind: "image",
  textContent: null,
  assetId: "asset-1",
  sourceApp: "paint.exe",
  byteSize: 68,
  ...overrides,
});
const commandError = (code: CommandError["code"]): CommandError => ({
  code,
  messageKey: `errors.${code}`,
  details: { reasonCode: "failed" },
  retryable: true,
});
const deferred = <T,>() => {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, reject, resolve };
};

let subscriptionRows: ClipboardItem[];
let deliverSnapshot: ((rows: ClipboardItem[]) => void) | undefined;

async function renderClipboard(initialKind: "all" | "text" | "image" = "all") {
  localStorage.setItem("aiceland.ui.language", "en-US");
  const user = userEvent.setup();
  const view = render(<I18nProvider><ClipboardPage initialKind={initialKind} /></I18nProvider>);
  await act(async () => { await Promise.resolve(); await Promise.resolve(); });
  return { ...view, user };
}

beforeEach(() => {
  subscriptionRows = [];
  deliverSnapshot = undefined;
  for (const mock of Object.values(mocks)) mock.mockReset();
  mocks.invoke.mockResolvedValue(undefined);
  mocks.retry.mockResolvedValue([]);
  mocks.confirm.mockReturnValue(true);
  mocks.createObjectURL.mockReturnValueOnce("blob:image-1").mockReturnValueOnce("blob:image-2");
  mocks.getClipboardAsset.mockResolvedValue({ assetId: "asset-1", mimeType: "image/png", base64: "AQID" });
  mocks.clearClipboardHistory.mockResolvedValue({ removedCount: 1 });
  mocks.deleteClipboardItem.mockResolvedValue({ id: "item-1", deleted: true });
  mocks.subscribeClipboardItems.mockImplementation(async (_input, _failure, snapshot) => {
    deliverSnapshot = snapshot;
    return { initial: subscriptionRows, retry: mocks.retry, dispose: mocks.dispose };
  });
  mocks.beginClipboardItemsSubscription.mockImplementation((_input, _failure, snapshot) => {
    deliverSnapshot = snapshot;
    return {
      ready: Promise.resolve({ initial: subscriptionRows, retry: mocks.retry, dispose: mocks.dispose }),
      dispose: mocks.dispose,
    };
  });
  Object.defineProperty(URL, "createObjectURL", { configurable: true, value: mocks.createObjectURL });
  Object.defineProperty(URL, "revokeObjectURL", { configurable: true, value: mocks.revokeObjectURL });
  vi.stubGlobal("confirm", mocks.confirm);
});

afterEach(() => {
  cleanup();
  localStorage.clear();
  vi.clearAllTimers();
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("clipboard history page", () => {
  it("subscribes with the locked payload and renders pinned rows first without translating content", async () => {
    subscriptionRows = [
      itemFixture({ id: "plain", textContent: "C:\\Build\\release", sourceApp: null }),
      itemFixture({ id: "pinned", textContent: "\\\\server\\share\\artifact", sourceApp: "Build Runner", pinned: true }),
    ];
    await renderClipboard("all");

    expect(mocks.beginClipboardItemsSubscription).toHaveBeenCalledWith(
      { query: "", contentKind: "all", limit: 500 },
      expect.any(Function),
      expect.any(Function),
    );
    expect(screen.getByRole("heading", { name: "Clipboard" })).toBeVisible();
    const cards = screen.getAllByTestId(/^clipboard-item-/);
    expect(cards.map((card) => card.dataset.testid)).toEqual(["clipboard-item-pinned", "clipboard-item-plain"]);
    expect(screen.getByText("\\\\server\\share\\artifact")).toBeVisible();
    expect(screen.getByText("C:\\Build\\release")).toBeVisible();
    expect(screen.getByText("Build Runner")).toBeVisible();
    expect(screen.getByText("Unknown source")).toBeVisible();
  });

  it("debounces search for 250 ms and resubscribes immediately for kind changes", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    await renderClipboard();
    const search = screen.getByRole("searchbox", { name: "Search clipboard history" });
    fireEvent.change(search, { target: { value: "build" } });
    await vi.advanceTimersByTimeAsync(249);
    expect(mocks.beginClipboardItemsSubscription).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1);
    await waitFor(() => expect(mocks.beginClipboardItemsSubscription).toHaveBeenLastCalledWith(
      { query: "build", contentKind: "all", limit: 500 }, expect.any(Function), expect.any(Function),
    ));
    fireEvent.click(screen.getByRole("button", { name: "Images" }));
    await waitFor(() => expect(mocks.beginClipboardItemsSubscription).toHaveBeenLastCalledWith(
      { query: "build", contentKind: "image", limit: 500 }, expect.any(Function), expect.any(Function),
    ));
  });

  it("keeps an image record when its thumbnail cannot be read and retries", async () => {
    subscriptionRows = [imageFixture()];
    mocks.getClipboardAsset
      .mockRejectedValueOnce(commandError("ioFailure"))
      .mockResolvedValueOnce({ assetId: "asset-1", mimeType: "image/png", base64: "AQID" });
    const { user } = await renderClipboard("image");

    expect(await screen.findByText("Unable to read the image. The original record was kept.")).toBeVisible();
    expect(screen.getByTestId("clipboard-item-image-1")).toBeInTheDocument();
    expect(mocks.deleteClipboardItem).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByRole("img", { name: "Clipboard image" })).toHaveAttribute("src", "blob:image-1");
  });

  it("revokes thumbnail object URLs on asset replacement, row removal, and unmount", async () => {
    subscriptionRows = [imageFixture()];
    const view = await renderClipboard("image");
    expect(await screen.findByRole("img", { name: "Clipboard image" })).toHaveAttribute("src", "blob:image-1");

    await act(async () => { deliverSnapshot?.([imageFixture({ assetId: "asset-2" })]); });
    await waitFor(() => expect(mocks.getClipboardAsset).toHaveBeenLastCalledWith({ assetId: "asset-2" }));
    expect(mocks.revokeObjectURL).toHaveBeenCalledWith("blob:image-1");
    expect(await screen.findByRole("img", { name: "Clipboard image" })).toHaveAttribute("src", "blob:image-2");

    await act(async () => { deliverSnapshot?.([]); });
    await waitFor(() => expect(mocks.revokeObjectURL).toHaveBeenCalledWith("blob:image-2"));
    view.unmount();
    expect(mocks.revokeObjectURL).toHaveBeenCalledTimes(2);
  });

  it("updates copy and pin state only from backend-confirmed rows", async () => {
    const original = itemFixture({ textContent: "Original" });
    subscriptionRows = [original];
    const copyResult = deferred<ClipboardItem>();
    const pinResult = deferred<ClipboardItem>();
    mocks.copyClipboardItem.mockReturnValue(copyResult.promise);
    mocks.setClipboardPinned.mockReturnValue(pinResult.promise);
    const { user } = await renderClipboard();

    const card = screen.getByTestId("clipboard-item-item-1");
    await user.click(within(card).getByRole("button", { name: "Copy — Original" }));
    expect(within(card).getByText("Original")).toBeVisible();
    expect(within(card).getByRole("button", { name: "Pin — Original" })).toBeEnabled();
    copyResult.resolve(itemFixture({ textContent: "Backend copy", lastSeenAt: 99 }));
    expect(await screen.findByText("Backend copy")).toBeVisible();
    expect(screen.getByText("Copied to clipboard")).toBeVisible();

    await user.click(screen.getByRole("button", { name: "Pin — Backend copy" }));
    expect(screen.getByRole("button", { name: "Pin — Backend copy" })).toBeDisabled();
    pinResult.resolve(itemFixture({ textContent: "Backend pin", pinned: true }));
    expect(await screen.findByRole("button", { name: "Unpin — Backend pin" })).toBeEnabled();
  });

  it("uses exact delete confirmation and retains the row until backend success", async () => {
    subscriptionRows = [itemFixture({ textContent: "Delete me" })];
    const deletion = deferred<{ id: string; deleted: true }>();
    mocks.deleteClipboardItem.mockReturnValue(deletion.promise);
    const { user } = await renderClipboard();

    await user.click(screen.getByRole("button", { name: "Delete — Delete me" }));
    expect(mocks.confirm).toHaveBeenCalledWith("Delete this AIceLand clipboard item?");
    expect(screen.getByTestId("clipboard-item-item-1")).toBeInTheDocument();
    deletion.resolve({ id: "item-1", deleted: true });
    await waitFor(() => expect(screen.queryByTestId("clipboard-item-item-1")).not.toBeInTheDocument());
  });

  it("clears only the confirmed range after backend success", async () => {
    subscriptionRows = [
      itemFixture({ id: "pinned", textContent: "Keep", pinned: true }),
      itemFixture({ id: "plain", textContent: "Remove" }),
    ];
    const clearing = deferred<{ removedCount: number }>();
    mocks.clearClipboardHistory.mockReturnValue(clearing.promise);
    const { user } = await renderClipboard();

    await user.click(screen.getByRole("button", { name: "Clear unpinned only" }));
    expect(mocks.confirm).toHaveBeenCalledWith("Clear AIceLand clipboard history in the selected scope?");
    expect(mocks.clearClipboardHistory).toHaveBeenCalledWith({ keepPinned: true });
    expect(screen.getByText("Remove")).toBeVisible();
    clearing.resolve({ removedCount: 1 });
    await waitFor(() => expect(screen.queryByText("Remove")).not.toBeInTheDocument());
    expect(screen.getByText("Keep")).toBeVisible();
  });

  it("gives icon controls labels, titles, and hover/focus tooltips", async () => {
    subscriptionRows = [itemFixture({ textContent: "Accessible" })];
    const { user } = await renderClipboard();
    const copy = screen.getByRole("button", { name: "Copy — Accessible" });
    const pin = screen.getByRole("button", { name: "Pin — Accessible" });
    const remove = screen.getByRole("button", { name: "Delete — Accessible" });
    expect(copy).toHaveAttribute("title", "Copy");
    expect(pin).toHaveAttribute("title", "Pin");
    expect(remove).toHaveAttribute("title", "Delete");

    await user.hover(copy);
    expect(await screen.findByRole("tooltip", { name: "Copy" })).toBeVisible();
    await user.unhover(copy);
    fireEvent.focus(pin);
    expect(await screen.findByRole("tooltip", { name: "Pin" })).toBeVisible();
    fireEvent.blur(pin);
    await waitFor(() => expect(screen.queryByRole("tooltip")).not.toBeInTheDocument());
  });

  it("disposes a pending subscription immediately on unmount", async () => {
    const ready = deferred<{ initial: ClipboardItem[]; retry: () => Promise<void>; dispose: () => void }>();
    const disposePending = vi.fn();
    mocks.beginClipboardItemsSubscription.mockReturnValue({ ready: ready.promise, dispose: disposePending });

    const view = await renderClipboard();
    view.unmount();

    expect(mocks.beginClipboardItemsSubscription).toHaveBeenCalledTimes(1);
    expect(disposePending).toHaveBeenCalledTimes(1);
    ready.resolve({ initial: [itemFixture({ textContent: "Too late" })], retry: mocks.retry, dispose: mocks.dispose });
    await act(async () => { await Promise.resolve(); });
    expect(screen.queryByText("Too late")).not.toBeInTheDocument();
  });

  it("disposes the old pending subscription immediately when the filter changes", async () => {
    const oldReady = deferred<{ initial: ClipboardItem[]; retry: () => Promise<void>; dispose: () => void }>();
    const disposeOld = vi.fn();
    mocks.beginClipboardItemsSubscription.mockImplementationOnce(() => ({ ready: oldReady.promise, dispose: disposeOld }));
    const { user } = await renderClipboard();

    await user.click(screen.getByRole("button", { name: "Images" }));
    expect(disposeOld).toHaveBeenCalledTimes(1);
    expect(mocks.beginClipboardItemsSubscription).toHaveBeenCalledTimes(2);
    oldReady.resolve({ initial: [itemFixture({ textContent: "Old view" })], retry: mocks.retry, dispose: mocks.dispose });
    await act(async () => { await Promise.resolve(); });
    expect(screen.queryByText("Old view")).not.toBeInTheDocument();
  });

  it("offers a local Retry that reloads a degraded listener subscription", async () => {
    const retryDegraded = vi.fn().mockResolvedValue(undefined);
    mocks.beginClipboardItemsSubscription.mockImplementation((_input, listenerFailure, snapshot) => {
      listenerFailure(commandError("sourceUnavailable"));
      snapshot([itemFixture({ textContent: "Degraded initial" })]);
      return {
        ready: Promise.resolve({
          initial: [itemFixture({ textContent: "Degraded initial" })],
          listenerState: "degraded",
          retry: retryDegraded,
          dispose: mocks.dispose,
        }),
        dispose: mocks.dispose,
      };
    });
    const { user } = await renderClipboard();

    expect(await screen.findByText("Clipboard monitoring is temporarily unavailable")).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Retry" }));
    expect(retryDegraded).toHaveBeenCalledTimes(1);
    expect(mocks.beginClipboardItemsSubscription).toHaveBeenCalledTimes(1);
  });

  it("keeps the degraded Retry available after a successful row action", async () => {
    mocks.beginClipboardItemsSubscription.mockImplementation((_input, listenerFailure) => {
      listenerFailure(commandError("sourceUnavailable"));
      return {
        ready: Promise.resolve({
          initial: [itemFixture({ textContent: "Degraded action row" })],
          listenerState: "degraded",
          retry: mocks.retry,
          dispose: mocks.dispose,
        }),
        dispose: mocks.dispose,
      };
    });
    mocks.setClipboardPinned.mockResolvedValue(itemFixture({ textContent: "Pinned while degraded", pinned: true }));
    const { user } = await renderClipboard();

    expect(await screen.findByRole("button", { name: "Retry" })).toBeEnabled();
    await user.click(screen.getByRole("button", { name: "Pin — Degraded action row" }));
    expect(await screen.findByRole("button", { name: "Unpin — Pinned while degraded" })).toBeEnabled();
    expect(screen.getByText("Clipboard monitoring is temporarily unavailable")).toBeVisible();
    expect(screen.getByRole("button", { name: "Retry" })).toBeEnabled();
  });

  it("rebuilds the current subscription from Retry after initial ready rejects", async () => {
    mocks.beginClipboardItemsSubscription
      .mockReturnValueOnce({ ready: Promise.reject(commandError("databaseFailure")), dispose: mocks.dispose })
      .mockReturnValueOnce({
        ready: Promise.resolve({
          initial: [itemFixture({ textContent: "Recovered after retry" })],
          listenerState: "active",
          retry: mocks.retry,
          dispose: mocks.dispose,
        }),
        dispose: mocks.dispose,
      });
    const { user } = await renderClipboard();

    expect(await screen.findByText("Clipboard action failed. Try again.")).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Retry" }));

    expect(await screen.findByText("Recovered after retry")).toBeVisible();
    expect(mocks.beginClipboardItemsSubscription).toHaveBeenCalledTimes(2);
  });

  it("invalidates a pending Retry when a new filtered subscription starts", async () => {
    const oldRetry = deferred<void>();
    const newRetry = vi.fn().mockResolvedValue(undefined);
    mocks.beginClipboardItemsSubscription
      .mockImplementationOnce((_input, listenerFailure) => {
        listenerFailure(commandError("sourceUnavailable"));
        return {
          ready: Promise.resolve({
            initial: [itemFixture({ textContent: "Old degraded view" })],
            listenerState: "degraded",
            retry: () => oldRetry.promise,
            dispose: mocks.dispose,
          }),
          dispose: mocks.dispose,
        };
      })
      .mockImplementationOnce((_input, listenerFailure) => {
        listenerFailure(commandError("sourceUnavailable"));
        return {
          ready: Promise.resolve({
            initial: [imageFixture({ id: "new-view-image" })],
            listenerState: "degraded",
            retry: newRetry,
            dispose: mocks.dispose,
          }),
          dispose: mocks.dispose,
        };
      });
    const { user } = await renderClipboard();

    await user.click(await screen.findByRole("button", { name: "Retry" }));
    await user.click(screen.getByRole("button", { name: "Images" }));
    const currentRetry = await screen.findByRole("button", { name: "Retry" });
    expect(currentRetry).toBeEnabled();
    oldRetry.reject(commandError("databaseFailure"));
    await act(async () => { await Promise.resolve(); });
    expect(screen.getByText("Clipboard monitoring is temporarily unavailable")).toBeVisible();
    expect(screen.queryByText("Clipboard action failed. Try again.")).not.toBeInTheDocument();

    await user.click(currentRetry);
    expect(newRetry).toHaveBeenCalledTimes(1);
  });

  it("keeps the newest overlapping row mutation when command results resolve out of order", async () => {
    subscriptionRows = [itemFixture({ textContent: "Original" })];
    const copyResult = deferred<ClipboardItem>();
    const pinResult = deferred<ClipboardItem>();
    mocks.copyClipboardItem.mockReturnValue(copyResult.promise);
    mocks.setClipboardPinned.mockReturnValue(pinResult.promise);
    const { user } = await renderClipboard();

    await user.click(screen.getByRole("button", { name: "Copy — Original" }));
    await user.click(screen.getByRole("button", { name: "Pin — Original" }));
    pinResult.resolve(itemFixture({ textContent: "Newest pin", pinned: true }));
    expect(await screen.findByRole("button", { name: "Unpin — Newest pin" })).toBeEnabled();
    copyResult.resolve(itemFixture({ textContent: "Late copy", pinned: false, lastSeenAt: 99 }));
    await act(async () => { await Promise.resolve(); });

    expect(screen.getByRole("button", { name: "Unpin — Newest pin" })).toBeEnabled();
    expect(screen.queryByText("Late copy")).not.toBeInTheDocument();
  });

  it("does not let an older subscription snapshot roll back a confirmed row mutation", async () => {
    const original = itemFixture({ textContent: "Before pin", pinned: false, lastSeenAt: 50 });
    subscriptionRows = [original];
    mocks.setClipboardPinned.mockResolvedValue(itemFixture({ textContent: "Confirmed pin", pinned: true, lastSeenAt: 50 }));
    const { user } = await renderClipboard();

    await user.click(screen.getByRole("button", { name: "Pin — Before pin" }));
    expect(await screen.findByRole("button", { name: "Unpin — Confirmed pin" })).toBeEnabled();
    await act(async () => { deliverSnapshot?.([original]); });

    expect(screen.getByRole("button", { name: "Unpin — Confirmed pin" })).toBeEnabled();
    expect(screen.queryByText("Before pin")).not.toBeInTheDocument();
  });

  it("lets a pending pin result win after an older snapshot arrives first", async () => {
    const original = itemFixture({ textContent: "Before pending pin", pinned: false, lastSeenAt: 50 });
    subscriptionRows = [original];
    const pinResult = deferred<ClipboardItem>();
    mocks.setClipboardPinned.mockReturnValue(pinResult.promise);
    const { user } = await renderClipboard();

    await user.click(screen.getByRole("button", { name: "Pin — Before pending pin" }));
    await act(async () => { deliverSnapshot?.([original]); });
    pinResult.resolve(itemFixture({ textContent: "Confirmed after snapshot", pinned: true, lastSeenAt: 50 }));

    expect(await screen.findByRole("button", { name: "Unpin — Confirmed after snapshot" })).toBeEnabled();
    expect(screen.queryByText("Before pending pin")).not.toBeInTheDocument();
  });

  it("does not resurrect a row from a late action or stale snapshot after delete", async () => {
    subscriptionRows = [itemFixture({ textContent: "Delete race" })];
    const copyResult = deferred<ClipboardItem>();
    mocks.copyClipboardItem.mockReturnValue(copyResult.promise);
    mocks.deleteClipboardItem.mockResolvedValue({ id: "item-1", deleted: true });
    const { user } = await renderClipboard();

    await user.click(screen.getByRole("button", { name: "Copy — Delete race" }));
    await user.click(screen.getByRole("button", { name: "Delete — Delete race" }));
    await waitFor(() => expect(screen.queryByTestId("clipboard-item-item-1")).not.toBeInTheDocument());
    copyResult.resolve(itemFixture({ textContent: "Late resurrection" }));
    await act(async () => { deliverSnapshot?.([itemFixture({ textContent: "Stale snapshot" })]); await Promise.resolve(); });

    expect(screen.queryByTestId("clipboard-item-item-1")).not.toBeInTheDocument();
    expect(screen.queryByText("Late resurrection")).not.toBeInTheDocument();
    expect(screen.queryByText("Stale snapshot")).not.toBeInTheDocument();
  });

  it("does not restore cleared rows or leak an old action into a new filter", async () => {
    subscriptionRows = [itemFixture({ textContent: "Old text" })];
    const copyResult = deferred<ClipboardItem>();
    mocks.copyClipboardItem.mockReturnValue(copyResult.promise);
    const { user } = await renderClipboard();

    await user.click(screen.getByRole("button", { name: "Copy — Old text" }));
    subscriptionRows = [imageFixture()];
    await user.click(screen.getByRole("button", { name: "Images" }));
    expect(await screen.findByRole("img", { name: "Clipboard image" })).toBeVisible();
    copyResult.resolve(itemFixture({ textContent: "Cross-filter text" }));
    await act(async () => { await Promise.resolve(); });
    expect(screen.queryByText("Cross-filter text")).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Clear history" }));
    await waitFor(() => expect(screen.queryByTestId("clipboard-item-image-1")).not.toBeInTheDocument());
    await act(async () => { deliverSnapshot?.([imageFixture()]); });
    expect(screen.queryByTestId("clipboard-item-image-1")).not.toBeInTheDocument();
  });

  it("keeps a newly captured row that arrives while clear is pending", async () => {
    const original = itemFixture({ id: "old", textContent: "Clear target" });
    const arrivedLater = itemFixture({ id: "new", textContent: "Captured during clear", lastSeenAt: 80 });
    subscriptionRows = [original];
    const clearing = deferred<{ removedCount: number }>();
    mocks.clearClipboardHistory.mockReturnValue(clearing.promise);
    const { user } = await renderClipboard();

    await user.click(screen.getByRole("button", { name: "Clear history" }));
    await act(async () => { deliverSnapshot?.([original, arrivedLater]); });
    expect(screen.getByText("Captured during clear")).toBeVisible();
    clearing.resolve({ removedCount: 1 });

    await waitFor(() => expect(screen.queryByText("Clear target")).not.toBeInTheDocument());
    expect(screen.getByText("Captured during clear")).toBeVisible();
  });

  it("serializes clear operations against every row mutation", async () => {
    subscriptionRows = [itemFixture({ textContent: "Serialized row" })];
    const clearing = deferred<{ removedCount: number }>();
    const pinning = deferred<ClipboardItem>();
    mocks.clearClipboardHistory.mockReturnValue(clearing.promise);
    mocks.setClipboardPinned.mockReturnValue(pinning.promise);
    const { user } = await renderClipboard();

    await user.click(screen.getByRole("button", { name: "Clear unpinned only" }));
    expect(screen.getByRole("button", { name: "Pin — Serialized row" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Pin — Serialized row" }));
    expect(mocks.setClipboardPinned).not.toHaveBeenCalled();
    clearing.reject(commandError("ioFailure"));
    await waitFor(() => expect(screen.getByRole("button", { name: "Pin — Serialized row" })).toBeEnabled());

    await user.click(screen.getByRole("button", { name: "Pin — Serialized row" }));
    expect(screen.getByRole("button", { name: "Clear unpinned only" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Clear history" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Clear history" }));
    expect(mocks.clearClipboardHistory).toHaveBeenCalledTimes(1);
    pinning.resolve(itemFixture({ textContent: "Serialized row", pinned: true }));
    expect(await screen.findByRole("button", { name: "Unpin — Serialized row" })).toBeEnabled();
  });

  it("reclaims completed mutation metadata while preserving overlap ordering", async () => {
    const pageModule = await import("./ClipboardPage") as unknown as {
      ClipboardMutationCoordinator: new () => {
        activeCount: number;
        begin: (id: string) => number;
        finish: (id: string, token: number) => void;
        invalidate: (ids: Iterable<string>) => void;
        isCurrent: (id: string, token: number) => boolean;
      };
    };
    const coordinator = new pageModule.ClipboardMutationCoordinator();
    const first = coordinator.begin("overlap");
    const second = coordinator.begin("overlap");
    expect(coordinator.isCurrent("overlap", first)).toBe(false);
    expect(coordinator.isCurrent("overlap", second)).toBe(true);
    coordinator.finish("overlap", first);
    expect(coordinator.activeCount).toBe(1);
    coordinator.invalidate(["overlap"]);
    expect(coordinator.activeCount).toBe(0);

    for (let index = 0; index < 1_000; index += 1) {
      const id = `historical-${index}`;
      const token = coordinator.begin(id);
      coordinator.finish(id, token);
    }
    expect(coordinator.activeCount).toBe(0);
  });

  it("bounds accessible action names without changing the verbatim card content", async () => {
    const largeContent = "😀".repeat(10_000);
    subscriptionRows = [itemFixture({ textContent: largeContent })];
    await renderClipboard();

    expect(screen.getByText(largeContent)).toBeVisible();
    const labels = screen.getAllByTestId("clipboard-item-item-1")[0]
      .querySelectorAll<HTMLButtonElement>(".clipboard-icon-button");
    expect(labels).toHaveLength(3);
    for (const button of labels) expect(button.getAttribute("aria-label")?.length).toBeLessThan(160);
  });
});
