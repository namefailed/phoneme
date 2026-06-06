// Vitest is configured with environment: "jsdom" in vite.config.ts,
// so document/window/localStorage are available globally.
import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";

// Stub the CSS import so Vitest doesn't choke on stylesheet syntax.
vi.mock("./modal.css", () => ({}));

const { confirmDelete } = await import("./ConfirmDelete");

function getOverlay() {
  // ConfirmDelete uses createRenderRoot() { return this; } so it renders to light DOM
  return document.querySelector("ph-confirm-delete")?.querySelector(".modal-overlay") || null;
}

function queryEl<T extends HTMLElement>(selector: string): T | null {
  // ConfirmDelete uses createRenderRoot() { return this; } so it renders to light DOM
  return document.querySelector("ph-confirm-delete")?.querySelector<T>(selector) || null;
}

beforeEach(() => {
  localStorage.clear();
  document.querySelectorAll("ph-confirm-delete").forEach((el) => el.remove());
});

afterEach(() => {
  document.querySelectorAll("ph-confirm-delete").forEach((el) => el.remove());
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
    await new Promise(r => setTimeout(r, 0));
    expect(getOverlay()).not.toBeNull();
    queryEl<HTMLButtonElement>("#btn-cancel")?.click();
    await promise;
  });

  it("resolves false when Cancel is clicked", async () => {
    const promise = confirmDelete();
    await new Promise(r => setTimeout(r, 0));
    queryEl<HTMLButtonElement>("#btn-cancel")?.click();
    expect(await promise).toBe(false);
    expect(getOverlay()).toBeNull();
  });

  it("resolves true when Delete is clicked", async () => {
    const promise = confirmDelete();
    await new Promise(r => setTimeout(r, 0));
    queryEl<HTMLButtonElement>("#btn-confirm")?.click();
    expect(await promise).toBe(true);
    expect(getOverlay()).toBeNull();
  });

  it("sets skip pref when 'Don't ask again' is checked before confirming", async () => {
    const promise = confirmDelete();
    await new Promise(r => setTimeout(r, 0));
    const cb = queryEl<HTMLInputElement>("#dont-ask-again")!;
    cb.checked = true;
    queryEl<HTMLButtonElement>("#btn-confirm")?.click();
    await promise;
    expect(localStorage.getItem("phoneme_skip_delete_confirm")).toBe("true");
  });

  it("does NOT set skip pref when checkbox is unchecked", async () => {
    const promise = confirmDelete();
    await new Promise(r => setTimeout(r, 0));
    queryEl<HTMLButtonElement>("#btn-confirm")?.click();
    await promise;
    expect(localStorage.getItem("phoneme_skip_delete_confirm")).toBeNull();
  });

  it("resolves false when Escape key is pressed", async () => {
    const promise = confirmDelete();
    await new Promise(r => setTimeout(r, 0));
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(await promise).toBe(false);
    expect(getOverlay()).toBeNull();
  });

  it("resolves false when clicking the overlay backdrop directly", async () => {
    const promise = confirmDelete();
    await new Promise(r => setTimeout(r, 0));
    const overlay = getOverlay() as HTMLElement;
    overlay.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(await promise).toBe(false);
  });
});

describe("confirmDelete — ConfirmDeleteOpts customisation", () => {
  it("shows a custom title in the modal header", async () => {
    const promise = confirmDelete({ title: 'Delete this tag?' });
    await new Promise(r => setTimeout(r, 0));
    expect(queryEl(".modal-title")?.textContent).toContain("Delete this tag?");
    queryEl<HTMLButtonElement>("#btn-cancel")?.click();
    await promise;
  });

  it("shows custom body text", async () => {
    const promise = confirmDelete({ body: 'This will remove it from all recordings.' });
    await new Promise(r => setTimeout(r, 0));
    expect(queryEl(".modal-body")?.textContent).toContain(
      "This will remove it from all recordings."
    );
    queryEl<HTMLButtonElement>("#btn-cancel")?.click();
    await promise;
  });

  it("shows a custom confirm button label", async () => {
    const promise = confirmDelete({ confirmLabel: 'Delete Tag' });
    await new Promise(r => setTimeout(r, 0));
    const btn = queryEl<HTMLButtonElement>("#btn-confirm");
    expect(btn?.textContent?.trim()).toBe("Delete Tag");
    btn?.click();
    await promise;
  });

  it("stores 'don't ask again' under the custom skipKey, not the default key", async () => {
    const promise = confirmDelete({ skipKey: 'phoneme_skip_tag_delete_confirm' });
    await new Promise(r => setTimeout(r, 0));
    const cb = queryEl<HTMLInputElement>("#dont-ask-again")!;
    cb.checked = true;
    queryEl<HTMLButtonElement>("#btn-confirm")?.click();
    await promise;
    expect(localStorage.getItem("phoneme_skip_tag_delete_confirm")).toBe("true");
    expect(localStorage.getItem("phoneme_skip_delete_confirm")).toBeNull();
  });

  it("custom skipKey pref does not affect the default skipKey (keys are isolated)", async () => {
    localStorage.setItem("phoneme_skip_tag_delete_confirm", "true");
    // Default skipKey ('phoneme_skip_delete_confirm') is not set, so the modal must appear.
    const promise = confirmDelete();
    await new Promise(r => setTimeout(r, 0));
    expect(getOverlay()).not.toBeNull();
    queryEl<HTMLButtonElement>("#btn-cancel")?.click();
    await promise;
  });

  it("custom skipKey bypasses the modal when already set to true", async () => {
    localStorage.setItem("phoneme_skip_tag_delete_confirm", "true");
    const result = await confirmDelete({ skipKey: "phoneme_skip_tag_delete_confirm" });
    expect(result).toBe(true);
    expect(getOverlay()).toBeNull();
  });

  it("uses default title/body/confirmLabel when opts are omitted", async () => {
    const promise = confirmDelete();
    await new Promise(r => setTimeout(r, 0));
    expect(queryEl(".modal-title")?.textContent).toContain("Delete Recording?");
    expect(queryEl(".modal-body")?.textContent).toContain("permanently delete");
    expect(queryEl<HTMLButtonElement>("#btn-confirm")?.textContent?.trim()).toBe("Delete");
    queryEl<HTMLButtonElement>("#btn-cancel")?.click();
    await promise;
  });
});
