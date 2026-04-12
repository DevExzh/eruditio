<script lang="ts">
  let {
    format,
    disabled = false,
    onclick,
  }: {
    format: string;
    disabled?: boolean;
    onclick: () => void;
  } = $props();
</script>

<button class="convert" {disabled} {onclick}>
  Convert to {format.toUpperCase()}
</button>

<style>
  .convert {
    width: 100%;
    padding: 12px;
    background: var(--accent);
    color: white;
    border: none;
    border-radius: 8px;
    font-size: 15px;
    font-weight: 600;
    cursor: pointer;
    transition: background 0.15s;
  }
  .convert:hover:not(:disabled) { background: var(--accent-hover); }
  .convert:disabled { opacity: 0.5; cursor: not-allowed; }
</style>
