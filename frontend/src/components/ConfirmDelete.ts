export function confirmDelete(): Promise<boolean> {
  return new Promise((resolve) => {
    if (localStorage.getItem("phoneme_skip_delete_confirm") === "true") {
      return resolve(true);
    }
    
    const overlay = document.createElement("div");
    overlay.className = "modal-overlay";
    overlay.style.position = "fixed";
    overlay.style.inset = "0";
    overlay.style.backgroundColor = "rgba(0,0,0,0.5)";
    overlay.style.zIndex = "9999";
    overlay.style.display = "flex";
    overlay.style.alignItems = "center";
    overlay.style.justifyContent = "center";
    
    overlay.innerHTML = `
      <div class="modal-dialog" style="background: var(--bg-surface); padding: 24px; border-radius: 8px; border: 1px solid var(--border-subtle); box-shadow: 0 10px 30px rgba(0,0,0,0.5); width: 320px;">
        <h3 style="margin: 0 0 12px 0; font-size: 16px; color: var(--fg-default);">Delete Recording?</h3>
        <p style="margin: 0 0 16px 0; font-size: 13px; color: var(--fg-muted);">Are you sure you want to delete this recording and its audio file?</p>
        <label style="display:flex; align-items:center; gap:8px; margin: 16px 0; cursor:pointer;">
          <input type="checkbox" id="dont-ask-again" />
          <span style="font-size: 13px; color: var(--fg-default);">Don't ask again</span>
        </label>
        <div style="display:flex; justify-content:flex-end; gap:8px;">
          <button id="btn-cancel" style="background: rgba(255,255,255,0.06); border: none; padding: 6px 12px; border-radius: 6px; color: var(--fg-default); cursor: pointer;">Cancel</button>
          <button class="danger" id="btn-confirm" style="background: var(--err); border: none; padding: 6px 12px; border-radius: 6px; color: white; cursor: pointer;">Delete</button>
        </div>
      </div>
    `;
    
    document.body.appendChild(overlay);
    
    overlay.querySelector("#btn-cancel")!.addEventListener("click", () => {
      document.body.removeChild(overlay);
      resolve(false);
    });
    
    overlay.querySelector("#btn-confirm")!.addEventListener("click", () => {
      const dontAsk = (overlay.querySelector("#dont-ask-again") as HTMLInputElement).checked;
      if (dontAsk) {
        localStorage.setItem("phoneme_skip_delete_confirm", "true");
      }
      document.body.removeChild(overlay);
      resolve(true);
    });
  });
}
