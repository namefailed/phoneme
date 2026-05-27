import { addTag, attachTag, detachTag, listTags, tagsFor, type Tag } from "../../services/ipc";
import { escapeHtml } from "../../utils/format";

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
    this.allTags = await listTags();
    this.attached = await tagsFor(this.recordingId);
    this.render();
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
        <datalist id="all-tags">${this.allTags.map((t) => `<option value="${t.name}">`).join("")}</datalist>
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
  }

  private async detach(tagId: number) {
    await detachTag(this.recordingId, tagId);
    await this.load();
  }

  private async attachByName(name: string) {
    let tag = this.allTags.find((t) => t.name === name);
    if (!tag) tag = await addTag(name);
    await attachTag(this.recordingId, tag.id);
    await this.load();
  }
}
