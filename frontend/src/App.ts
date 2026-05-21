export class App {
  private container: HTMLElement;

  constructor(container: HTMLElement) {
    this.container = container;
    this.render();
  }

  render() {
    this.container.innerHTML = `
      <div class="app-shell">
        <p>Phoneme — loading…</p>
      </div>
    `;
  }
}
