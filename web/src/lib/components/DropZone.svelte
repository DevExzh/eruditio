<script lang="ts">
  let { onFile }: { onFile: (file: File) => void } = $props();

  let dragging = $state(false);
  let inputEl: HTMLInputElement;

  function handleDrop(e: DragEvent) {
    e.preventDefault();
    dragging = false;
    const file = e.dataTransfer?.files[0];
    if (file) onFile(file);
  }

  function handleDragOver(e: DragEvent) {
    e.preventDefault();
    dragging = true;
  }

  function handleDragLeave() {
    dragging = false;
  }

  function handleClick() {
    inputEl.click();
  }

  function handleInput(e: Event) {
    const target = e.target as HTMLInputElement;
    const file = target.files?.[0];
    if (file) onFile(file);
  }
</script>

<div
  class="dropzone"
  class:dragging
  ondrop={handleDrop}
  ondragover={handleDragOver}
  ondragleave={handleDragLeave}
  onclick={handleClick}
  role="button"
  tabindex="0"
>
  <div class="icon">📄</div>
  <div class="text">
    Drop your ebook here or <span class="link">browse files</span>
  </div>
  <div class="hint">
    Supports EPUB, MOBI, FB2, AZW3, HTML, TXT, MD, and 25+ more formats
  </div>
  <input
    bind:this={inputEl}
    type="file"
    oninput={handleInput}
    style="display:none"
  />
</div>

<style>
  .dropzone {
    border: 2px dashed var(--border);
    border-radius: 12px;
    padding: 40px 24px;
    text-align: center;
    margin-bottom: 24px;
    background: var(--bg-card);
    cursor: pointer;
    transition: border-color 0.15s, background 0.15s;
  }
  .dropzone:hover,
  .dropzone.dragging {
    border-color: var(--accent);
    background: #1a2744;
  }
  .icon { font-size: 32px; margin-bottom: 8px; }
  .text { font-size: 15px; color: var(--text-muted); }
  .link { color: var(--accent); text-decoration: underline; }
  .hint { font-size: 12px; color: var(--text-faint); margin-top: 8px; }
</style>
