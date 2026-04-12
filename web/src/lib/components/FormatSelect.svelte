<script lang="ts">
  let {
    formats,
    value = $bindable("epub"),
  }: {
    formats: string[];
    value: string;
  } = $props();
</script>

<div class="format-select">
  <label for="output-format">Output format</label>
  <select id="output-format" bind:value>
    {#each formats as fmt (fmt)}
      <option value={fmt}>{fmt.toUpperCase()}</option>
    {/each}
  </select>
</div>

<style>
  .format-select { margin-bottom: 16px; }
  label {
    font-size: 13px;
    color: var(--text-muted);
    display: block;
    margin-bottom: 6px;
  }
  select {
    width: 100%;
    background: var(--bg-card);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 10px 14px;
    color: var(--text);
    font-size: 14px;
    appearance: none;
    cursor: pointer;
  }
  select:focus { outline: 1px solid var(--accent); }
</style>
