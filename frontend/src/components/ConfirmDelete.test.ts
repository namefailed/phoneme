// Vitest is configured with environment: "jsdom" in vite.config.ts,
// so document/window/localStorage are available globally.
import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";

// Stub the CSS import so Vitest doesn't choke on stylesheet syntax.
vi.mock("./modal.css", () => ({}));

const { confirmDelete } = await import("./ConfirmDelete");

function getOverlay() {
  return document.querySelector(".modal-overlay");
}

beforeEach(() => {
  localStorage.clear();
  document.querySelectorAll(".modal-overlay").forEach((el) => el.remove());
});

afterEach(() => {
  document.querySelectorAll(".modal-overlay").forEach((el) => el.remove());
});

describe("confirmDelete", () => {
  it("resolves true immediately when skip-pref is set", async () => {
    localStorage.setItem("phoneme_skip_delete_confirm", "true");
    const result = await confirmDelete();
    expect(result).toBe(true);
    expect(getOverlay()).toBeNull();
  });

  it("shows a modal when pref is not set", async () => {
    const promise = confirmDelete();
    expect(getOverlay()).not.toBeNull();
    (document.querySelector("#btn-cancel") as HTMLButtonElement)?.click();
    await promise;
  });

  it("resolves false when Cancel is clicked", async () => {
    const promise = confirmDelete();
    (document.querySelector("#btn-cancel") as HTMLButtonElement)?.click();
    expect(await promise).toBe(false);
    expect(getOverlay()).toBeNull();
  });

  it("resolves true when Delete is clicked", async () => {
    const promise = confirmDelete();
    (document.querySelector("#btn-confirm") as HTMLButtonElement)?.click();
    expect(await promise).toBe(true);
    expect(getOverlay()).toBeNull();
  });

  it("sets skip pref when 'Don't ask again' is checked before confirming", async () => {
    const promise = confirmDelete();
    const cb = document.querySelector<HTMLInputElement>("#dont-ask-again")!;
    cb.checked = true;
    (document.querySelector("#btn-confirm") as HTMLButtonElement)?.click();
    await promise;
    expect(localStorage.getItem("phoneme_skip_delete_confirm")).toBe("true");
  });

  it("does NOT set skip pref when checkbox is unchecked", async () => {
    const promise = confirmDelete();
    (document.querySelector("#btn-confirm") as HTMLButtonElement)?.click();
    await promise;
    expect(localStorage.getItem("phoneme_skip_delete_confirm")).toBeNull();
  });

  it("resolves false when Escape key is pressed", async () => {
    const promise = confirmDelete();
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(await promise).toBe(false);
    expect(getOverlay()).toBeNull();
  });

  it("resolves false when clicking the overlay backdrop directly", async () => {
    const promise = confirmDelete();
    const overlay = getOverlay() as HTMLElement;
    overlay.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(await promise).toBe(false);
  });
});
