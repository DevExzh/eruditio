<script lang="ts">
  let {
    name,
    size,
    format,
    onClear,
  }: {
    name: string;
    size: number;
    format: string;
    onClear: () => void;
  } = $props();

  function formatSize(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  }
</script>

<div class="file-info">
  <div class="details">
    <div class="name">📘 {name}</div>
    <div class="meta">{formatSize(size)} · {format.toUpperCase()} detected</div>
  </div>
  <button class="clear" onclick={onClear}>✕ Clear</button>
</div>

<style>
  .file-info {
    background: var(--bg-card);
    border-radius: 8px;
    padding: 12px 16px;
    margin-bottom: 16px;
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .name { font-size: 14px; color: var(--text); }
  .meta { font-size: 12px; color: var(--text-dim); }
  .clear {
    background: none;
    border: 1px solid var(--border);
    color: var(--text-muted);
    padding: 4px 12px;
    border-radius: 6px;
    font-size: 12px;
    cursor: pointer;
  }
  .clear:hover { border-color: var(--text-muted); }
</style>
