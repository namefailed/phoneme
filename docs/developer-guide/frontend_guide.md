# 🎨 Frontend Developer Guide

Phoneme's frontend is a single-page app built for maximum responsiveness, low memory footprint, and complete keyboard accessibility. It runs within Tauri's WebView and is built using **Vite**, **TypeScript**, **Lit**, and **Vanilla CSS**.

---

## 🏗️ 1. Rendering Model: Lit & The Light DOM

Instead of virtual DOM trees, Phoneme leverages **Lit** to compile and render HTML templates directly to the DOM using template literals.

### Custom Components
All UI views and widgets are Web Components inheriting from `LitElement` (e.g. `HeaderBar`, `ModelPicker`, `SavedSearches`). 

### The Shadow DOM vs. Light DOM Decision
By default, Lit encapsulates elements inside a **Shadow DOM**. However, Shadow DOM boundaries prevent global styles and class modifiers from reaching child nodes unless they are explicitly passed via CSS custom properties. 

To bypass these limits, Phoneme components override `createRenderRoot()` to render templates directly into the **Light DOM**:
```typescript
import { LitElement, html } from "lit";
import { customElement } from "lit/decorators.js";

@customElement("my-component")
export class MyComponent extends LitElement {
  override createRenderRoot() {
    return this; // Renders directly to Light DOM, ignoring the shadow boundary
  }

  render() {
    return html`<div class="my-style">Hello Phoneme!</div>`;
  }
}
```

> [!CAUTION]
> Because components render in the Light DOM, their class names and styling rules are global. Ensure you namespace CSS classes (e.g. prefixing classes with component abbreviations like `.hb-` for HeaderBar, `.rv-` for RecordingsView) to prevent layout leakage.

---

## 🔄 2. State Management & The Store

State is synchronized using a reactive `Store` pattern ([`store.ts`](file:///c:/Users/Namef/Projects/dev/phoneme/frontend/src/state/store.ts)):

```text
  [Daemon Event Broadcast] ──> [Tauri Event Subscriber]
                                         │
                                         ▼
                               [frontend/state/store.ts]
                                         │
                        (Notifies Subscribed Callbacks)
                                         │
                                         ▼
                                [Lit Components]
                               (Trigger Rerender)
```

### Subscribing to Store Updates
Lit components subscribe to state changes when mounted and clean up their event listeners when detached:
```typescript
import { store } from "../state/store";

export class MyComponent extends LitElement {
  private unsubscribe?: () => void;

  override connectedCallback() {
    super.connectedCallback();
    // Subscribe to catalog changes
    this.unsubscribe = store.subscribe((state) => {
      this.recordings = state.recordings;
      this.requestUpdate(); // Force Lit to refresh template
    });
  }

  override disconnectedCallback() {
    this.unsubscribe?.();
    super.disconnectedCallback();
  }
}
```

---

## ⌨️ 3. Keyboard Layer & Vim 2D Pane Navigation

Phoneme features an advanced, opt-in Vim navigation layout. The keyboard router is defined in [`keyboard.ts`](file:///c:/Users/Namef/Projects/dev/phoneme/frontend/src/services/keyboard.ts).

### 2D Grid Panes
The workspace is split into three core grid panes: **Sidebar** (filters + queue), **List** (recordings list), and **Detail** (open notes and transcripts).
- Moving between panes is triggered using the standard Vim keys `h` (left) and `l` (right).
- Moving within a pane is driven by `j` (down) and `k` (up).
- Focus indexes and layout bounds are updated in a central state; when focus switches to a pane, the active element receives the `.kbd-cursor` CSS class, drawing a distinct focus border.

### Double-KeyPress Chords ("g-chords")
To navigate quickly, Phoneme supports double-keypress Vim chords:
- `g l`: Go to the Library view.
- `g s`: Open Settings.
- `g d`: Move keyboard focus directly into the detail pane of the open recording.
- `g D`: Open the Doctor dashboard.
- `g T`: Open the Tag Manager.

### Cheat Sheet Registry
All key combos are self-documenting. If you add a new shortcut key inside [`keyboard.ts`](file:///c:/Users/Namef/Projects/dev/phoneme/frontend/src/services/keyboard.ts), make sure to add its details to the `BASE_HELP_GROUPS` array. Pressing **`?`** anywhere in the app reads this registry and displays a popup overlay of all available hotkeys.

---

## ⏱️ 4. Layout Transitions & Animation Speeds

Phoneme uses CSS transitions to animate pane sliding, sidebar collapses, and the overlay fade-ins.
- **Dynamic CSS Variable:** Transition durations are governed by the `--pane-anim` CSS custom property on the document root.
- **Configurable Speeds:** The animation speed is defined in the user settings (`interface.animation_speed` TOML property). The keyboard service reads this value at startup and sets `--pane-anim` to:
  - `"off"`: `0ms` (bypasses transition animations entirely, saving CPU cycles).
  - `"fast"`: `110ms`
  - `"normal"`: `200ms`
  - `"slow"`: `320ms`
