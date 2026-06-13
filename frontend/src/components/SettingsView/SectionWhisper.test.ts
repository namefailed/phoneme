import { describe, it, expect } from "vitest";
import {
  effectivePortFor,
  effectiveLocalWhisperHint,
  type WhisperPortStatus,
} from "./SectionWhisper";

describe("effectivePortFor — which port to show", () => {
  it("returns null when no status is available (daemon down)", () => {
    expect(effectivePortFor(5809, null)).toBeNull();
    expect(effectivePortFor(5809, undefined)).toBeNull();
  });

  it("returns null when the main server is on its preferred port", () => {
    const status: WhisperPortStatus = {
      whisper_preferred_port: 5809,
      whisper_effective_port: 5809,
    };
    expect(effectivePortFor(5809, status)).toBeNull();
  });

  it("reports the effective port + note when the main server fell back", () => {
    const status: WhisperPortStatus = {
      whisper_preferred_port: 5809,
      whisper_effective_port: 51234,
    };
    expect(effectivePortFor(5809, status)).toEqual({
      effective: 51234,
      preferred: 5809,
      note: "(running on 51234 — preferred 5809 was busy)",
    });
  });

  it("matches the preview server pair independently of the main one", () => {
    const status: WhisperPortStatus = {
      whisper_preferred_port: 5809,
      whisper_effective_port: 5809,
      preview_whisper_preferred_port: 5810,
      preview_whisper_effective_port: 49999,
    };
    // The configured port belongs to the preview server here.
    expect(effectivePortFor(5810, status)).toEqual({
      effective: 49999,
      preferred: 5810,
      note: "(running on 49999 — preferred 5810 was busy)",
    });
    // The main server is on its preferred port, so no note for it.
    expect(effectivePortFor(5809, status)).toBeNull();
  });

  it("returns null for a port that matches neither server", () => {
    const status: WhisperPortStatus = {
      whisper_preferred_port: 5809,
      whisper_effective_port: 51234,
    };
    expect(effectivePortFor(9999, status)).toBeNull();
  });

  it("returns null when the matching effective port is null (server not running)", () => {
    const status: WhisperPortStatus = {
      whisper_preferred_port: 5809,
      whisper_effective_port: null,
    };
    expect(effectivePortFor(5809, status)).toBeNull();
  });

  it("ignores a partial status missing the effective field", () => {
    const status: WhisperPortStatus = { whisper_preferred_port: 5809 };
    expect(effectivePortFor(5809, status)).toBeNull();
  });
});

describe("effectiveLocalWhisperHint — URL rewrite + note", () => {
  const fellBack: WhisperPortStatus = {
    whisper_preferred_port: 5809,
    whisper_effective_port: 51234,
  };

  it("rewrites a 127.0.0.1 URL to the effective port and supplies the note", () => {
    expect(effectiveLocalWhisperHint("http://127.0.0.1:5809", fellBack)).toEqual({
      url: "http://127.0.0.1:51234",
      note: "(running on 51234 — preferred 5809 was busy)",
    });
  });

  it("tolerates a trailing slash", () => {
    expect(effectiveLocalWhisperHint("http://127.0.0.1:5809/", fellBack)).toEqual({
      url: "http://127.0.0.1:51234",
      note: "(running on 51234 — preferred 5809 was busy)",
    });
  });

  it("leaves the URL untouched (no note) when no fallback applies", () => {
    const onPreferred: WhisperPortStatus = {
      whisper_preferred_port: 5809,
      whisper_effective_port: 5809,
    };
    expect(effectiveLocalWhisperHint("http://127.0.0.1:5809", onPreferred)).toEqual({
      url: "http://127.0.0.1:5809",
      note: "",
    });
    expect(effectiveLocalWhisperHint("http://127.0.0.1:5809", null)).toEqual({
      url: "http://127.0.0.1:5809",
      note: "",
    });
  });

  it("leaves a non-local (external) URL untouched", () => {
    expect(effectiveLocalWhisperHint("http://192.168.1.5:8080/inference", fellBack)).toEqual({
      url: "http://192.168.1.5:8080/inference",
      note: "",
    });
  });
});
