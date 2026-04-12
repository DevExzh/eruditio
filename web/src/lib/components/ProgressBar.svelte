<script lang="ts">
  let { stage }: { stage: string } = $props();

  const stages = ["Reading...", "Transforming...", "Writing...", "Done"];
  let progress = $derived.by(() => {
    const idx = stages.indexOf(stage);
    return idx >= 0 ? ((idx + 1) / stages.length) * 100 : 0;
  });
  let stepLabel = $derived.by(() => {
    const idx = stages.indexOf(stage);
    return idx >= 0 ? `Step ${idx + 1} of ${stages.length}` : "";
  });
</script>

<div class="progress">
  <div class="stage">
    <div class="spinner"></div>
    <span>{stage}</span>
  </div>
  <div class="bar">
    <div class="fill" style="width: {progress}%"></div>
  </div>
  <div class="step">{stepLabel}</div>
</div>

<style>
  .progress {
    margin-top: 20px;
    background: var(--bg-card);
    border-radius: 8px;
    padding: 16px;
  }
  .stage {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 10px;
    font-size: 14px;
  }
  .spinner {
    width: 16px;
    height: 16px;
    border: 2px solid var(--accent);
    border-top-color: transparent;
    border-radius: 50%;
    animation: spin 1s linear infinite;
  }
  @keyframes spin { to { transform: rotate(360deg); } }
  .bar {
    background: var(--bg);
    border-radius: 4px;
    height: 6px;
    overflow: hidden;
  }
  .fill {
    background: var(--accent);
    height: 100%;
    border-radius: 4px;
    transition: width 0.3s;
  }
  .step { font-size: 11px; color: var(--text-faint); margin-top: 6px; }
</style>
