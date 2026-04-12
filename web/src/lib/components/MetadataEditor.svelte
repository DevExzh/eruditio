<script lang="ts">
  import type { ConversionOptions } from "../types";

  let { options = $bindable({}) }: { options: ConversionOptions } = $props();
  let expanded = $state(false);
</script>

<div class="metadata">
  <button class="toggle" onclick={() => (expanded = !expanded)}>
    <span class="arrow">{expanded ? "▼" : "▶"}</span>
    <span>Metadata overrides</span>
    <span class="hint">(optional)</span>
  </button>

  {#if expanded}
    <div class="fields">
      <div class="row">
        <div class="field">
          <label for="meta-title">Title</label>
          <input id="meta-title" bind:value={options.title} placeholder="Override title..." />
        </div>
        <div class="field">
          <label for="meta-authors">Authors</label>
          <input id="meta-authors" bind:value={options.authors} placeholder="Override authors..." />
        </div>
      </div>
      <div class="row">
        <div class="field">
          <label for="meta-publisher">Publisher</label>
          <input id="meta-publisher" bind:value={options.publisher} placeholder="Override publisher..." />
        </div>
        <div class="field">
          <label for="meta-language">Language</label>
          <input id="meta-language" bind:value={options.language} placeholder="e.g. en, zh..." />
        </div>
      </div>
      <div class="row">
        <div class="field">
          <label for="meta-series">Series</label>
          <input id="meta-series" bind:value={options.series} placeholder="Series name..." />
        </div>
        <div class="field">
          <label for="meta-series-index">Series Index</label>
          <input
            id="meta-series-index"
            type="number"
            bind:value={options.seriesIndex}
            placeholder="e.g. 1, 2..."
          />
        </div>
      </div>
      <div class="field full">
        <label for="meta-description">Description</label>
        <textarea id="meta-description" bind:value={options.description} placeholder="Book description..."></textarea>
      </div>
    </div>
  {/if}
</div>

<style>
  .metadata { margin-bottom: 20px; }
  .toggle {
    display: flex;
    align-items: center;
    gap: 8px;
    background: none;
    border: none;
    cursor: pointer;
    padding: 8px 0;
    color: var(--text-muted);
    font-size: 13px;
  }
  .arrow { font-size: 12px; color: var(--text-dim); }
  .hint { font-size: 11px; color: var(--text-faint); }
  .fields { margin-top: 12px; }
  .row { display: grid; grid-template-columns: 1fr 1fr; gap: 12px; margin-bottom: 12px; }
  .field { display: flex; flex-direction: column; }
  .field.full { margin-bottom: 0; }
  label { font-size: 12px; color: var(--text-dim); margin-bottom: 4px; }
  input, textarea {
    background: var(--bg-card);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 8px 10px;
    color: var(--text);
    font-size: 13px;
    font-family: inherit;
  }
  input:focus, textarea:focus { outline: 1px solid var(--accent); }
  textarea { height: 60px; resize: vertical; }
</style>
