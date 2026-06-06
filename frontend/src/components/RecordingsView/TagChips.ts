import { addTag, attachTag, detachTag, listAllTags, tagsFor, type Tag } from "../../services/ipc";
import { escapeHtml, escapeAttr } from "../../utils/format";
import { showToast } from "../../utils/toast";

export class TagChips {
  private container: HTMLElement;
  private recordingId: string;
  private attached: Tag[] = [];
  private allTags: Tag[] = [];

  constructor(container: HTMLElement, recordingId: string) {
    this.container = container;
    this.recordingId = recordingId;
    void this.load();
  }

  private async load() {
    try {
      this.allTags = await listAllTags();
      this.attached = await tagsFor(this.recordingId);
      this.render();
    } catch (e) {
      showToast(`Failed to load tags: ${e}`, "error");
    }
  }

  private render() {
    const chips = this.attached
      .map(
        (t) => `<span class="tag-chip" data-tag-id="${t.id}" style="${
          t.color ? `--tag-color: ${t.color}` : ""
        }">${escapeHtml(t.name)} <button class="tag-x">×</button></span>`
      )
      .join("");
    this.container.innerHTML = `
      <div class="tags">
        ${chips}
        <input class="tag-add" placeholder="+ add tag" list="all-tags" />
        <datalist id="all-tags">${this.allTags.map((t) => `<option value="${escapeAttr(t.name)}">`).join("")}</datalist>
        <button class="tag-manage" title="Create, rename, recolor, and delete tags">🏷 Manage tags</button>
      </div>
    `;
    this.container.querySelectorAll<HTMLButtonElement>(".tag-x").forEach((btn) => {
      btn.addEventListener("click", () => {
        const id = Number(btn.parentElement!.getAttribute("data-tag-id"));
        void this.detach(id);
      });
    });
    const input = this.container.querySelector<HTMLInputElement>(".tag-add");
    input?.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        const name = input.value.trim();
        if (name) void this.attachByName(name);
      }
    });
    const manageBtn = this.container.querySelector<HTMLButtonElement>(".tag-manage");
    manageBtn?.addEventListener("click", async () => {
      const { openTagManager } = await import("../TagManager");
      await openTagManager();
      // Tags may have been renamed/recolored/deleted — refresh chips + datalist.
      await this.load();
    });
  }

  private async detach(tagId: number) {
    try {
      await detachTag(this.recordingId, tagId);
      await this.load();
    } catch (e) {
      showToast(`Failed to remove tag: ${e}`, "error");
    }
  }

  private async attachByName(name: string) {
    try {
      let tag = this.allTags.find((t) => t.name === name);
      if (!tag) tag = await addTag(name);
      await attachTag(this.recordingId, tag.id);
      const input = this.container.querySelector<HTMLInputElement>(".tag-add");
      if (input) input.value = "";
      await this.load();
    } catch (e) {
      showToast(`Failed to add tag: ${e}`, "error");
    }
  }
}
