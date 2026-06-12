import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { showToast } from "./toast";

function getContainer() {
  return document.getElementById("toast-container");
}

function getAllToasts() {
  return document.querySelectorAll(".toast");
}

beforeEach(() => {
  document.getElementById("toast-container")?.remove();
});

afterEach(() => {
  document.getElementById("toast-container")?.remove();
  vi.useRealTimers();
});

describe("showToast", () => {
  it("creates #toast-container on the first call if it does not exist", () => {
    expect(getContainer()).toBeNull();
    showToast("hello", "info");
    expect(getContainer()).not.toBeNull();
  });

  it("reuses the existing #toast-container", () => {
    showToast("first", "info");
    showToast("second", "info");
    expect(document.querySelectorAll("#toast-container").length).toBe(1);
    expect(getAllToasts().length).toBe(2);
  });

  it("applies the correct type class for each toast type", () => {
    showToast("s", "success");
    showToast("e", "error");
    showToast("w", "warning");
    showToast("i", "info");
    const toasts = getAllToasts();
    expect(toasts[0].classList.contains("toast-success")).toBe(true);
    expect(toasts[1].classList.contains("toast-error")).toBe(true);
    expect(toasts[2].classList.contains("toast-warning")).toBe(true);
    expect(toasts[3].classList.contains("toast-info")).toBe(true);
  });

  it("sets role=alert for screen reader accessibility", () => {
    showToast("accessible", "info");
    expect(document.querySelector(".toast")!.getAttribute("role")).toBe("alert");
  });

  it("HTML-escapes the message to prevent XSS", () => {
    showToast("<script>alert('xss')</script>", "info");
    const msg = document.querySelector(".toast-msg")!;
    expect(msg.innerHTML).toBe("&lt;script&gt;alert('xss')&lt;/script&gt;");
    expect(msg.textContent).toBe("<script>alert('xss')</script>");
  });

  it("escapes ampersands in the message", () => {
    showToast("fish & chips", "info");
    expect(document.querySelector(".toast-msg")!.innerHTML).toBe("fish &amp; chips");
  });

  it("close button adds the toast-out class to trigger the fade animation", () => {
    showToast("closable", "info");
    const toast = document.querySelector(".toast")!;
    toast.querySelector<HTMLButtonElement>(".toast-close")!.click();
    expect(toast.classList.contains("toast-out")).toBe(true);
  });

  it("toast is removed from DOM after close click and animationend fires", () => {
    showToast("removable", "info");
    const toast = document.querySelector(".toast")!;
    toast.querySelector<HTMLButtonElement>(".toast-close")!.click();
    toast.dispatchEvent(new Event("animationend"));
    expect(toast.isConnected).toBe(false);
  });

  it("clicking close on an already-removed toast is a no-op (no error)", () => {
    showToast("gone", "info");
    const toast = document.querySelector(".toast")!;
    toast.querySelector<HTMLButtonElement>(".toast-close")!.click();
    toast.dispatchEvent(new Event("animationend")); // removes it
    // Second animationend (or click) should not throw
    expect(() => toast.dispatchEvent(new Event("animationend"))).not.toThrow();
  });

  it("success toast starts the auto-dismiss timer after 3000ms", () => {
    vi.useFakeTimers();
    showToast("auto", "success");
    const toast = document.querySelector(".toast")!;
    expect(toast.classList.contains("toast-out")).toBe(false);
    vi.advanceTimersByTime(3000);
    expect(toast.classList.contains("toast-out")).toBe(true);
  });

  it("info toast starts the auto-dismiss timer after 3500ms", () => {
    vi.useFakeTimers();
    showToast("info msg", "info");
    const toast = document.querySelector(".toast")!;
    vi.advanceTimersByTime(3499);
    expect(toast.classList.contains("toast-out")).toBe(false);
    vi.advanceTimersByTime(1);
    expect(toast.classList.contains("toast-out")).toBe(true);
  });

  it("warning toast starts the auto-dismiss timer after 6000ms", () => {
    vi.useFakeTimers();
    showToast("warn msg", "warning");
    const toast = document.querySelector(".toast")!;
    vi.advanceTimersByTime(5999);
    expect(toast.classList.contains("toast-out")).toBe(false);
    vi.advanceTimersByTime(1);
    expect(toast.classList.contains("toast-out")).toBe(true);
  });

  it("error toast auto-dismisses after its long window (10s)", () => {
    // Errors used to persist forever; now they time out like everything else
    // (hover pausing the clock is what protects "I was reading it").
    vi.useFakeTimers();
    showToast("expires", "error");
    const toast = document.querySelector(".toast")!;
    vi.advanceTimersByTime(9_999);
    expect(toast.classList.contains("toast-out")).toBe(false);
    vi.advanceTimersByTime(1);
    expect(toast.classList.contains("toast-out")).toBe(true);
  });

  it("hovering pauses the auto-dismiss clock; leaving resumes it", () => {
    vi.useFakeTimers();
    showToast("hover me", "success"); // 3000ms window
    const toast = document.querySelector<HTMLElement>(".toast")!;
    vi.advanceTimersByTime(2000);
    toast.dispatchEvent(new MouseEvent("mouseenter"));
    // The clock is paused — far past the original deadline, still alive.
    vi.advanceTimersByTime(60_000);
    expect(toast.classList.contains("toast-out")).toBe(false);
    toast.dispatchEvent(new MouseEvent("mouseleave"));
    // ~1000ms remained when paused; the resume grace floor is 800ms.
    vi.advanceTimersByTime(999);
    expect(toast.classList.contains("toast-out")).toBe(false);
    vi.advanceTimersByTime(1);
    expect(toast.classList.contains("toast-out")).toBe(true);
  });

  it("renders a countdown bar on timed toasts but not sticky ones", () => {
    showToast("timed", "info");
    showToast("sticky", "warning", 0);
    const toasts = document.querySelectorAll(".toast");
    expect(toasts[0].querySelector(".toast-countdown")).not.toBeNull();
    expect(toasts[1].querySelector(".toast-countdown")).toBeNull();
  });

  it("caps the stack: a burst drops the oldest toast", () => {
    for (let i = 0; i < 8; i++) showToast(`msg ${i}`, "info", 0);
    const msgs = [...document.querySelectorAll(".toast-msg")].map((el) => el.textContent);
    expect(msgs.length).toBe(6);
    expect(msgs[0]).toBe("msg 2"); // 0 and 1 were dropped
    expect(msgs[5]).toBe("msg 7");
  });

  it("custom duration overrides the type default", () => {
    vi.useFakeTimers();
    showToast("custom", "error", 1000); // shorter than the error default
    const toast = document.querySelector(".toast")!;
    vi.advanceTimersByTime(999);
    expect(toast.classList.contains("toast-out")).toBe(false);
    vi.advanceTimersByTime(1);
    expect(toast.classList.contains("toast-out")).toBe(true);
  });

  it("duration=0 keeps any type from auto-dismissing", () => {
    vi.useFakeTimers();
    showToast("persist success", "success", 0);
    const toast = document.querySelector(".toast")!;
    vi.advanceTimersByTime(30_000);
    expect(toast.classList.contains("toast-out")).toBe(false);
  });
});
