<script setup lang="ts">
import type { ProductMetricsAvailable } from "../product/types";

withDefaults(
  defineProps<{
    metrics: ProductMetricsAvailable | null;
    showTotals?: boolean;
    compact?: boolean;
  }>(),
  {
    showTotals: false,
    compact: false,
  },
);

const displayMetric = (value: string | null | undefined): string => value ?? "—";
</script>

<template>
  <div
    class="traffic-metrics-panel"
    :class="{ 'traffic-metrics-panel--compact': compact }"
    data-testid="traffic-metrics-panel"
    aria-label="上下行网络指标"
  >
    <div
      class="traffic-metric traffic-metric--upload"
      :class="{ 'traffic-metric--with-totals': showTotals }"
      data-testid="traffic-metric-upload"
      :aria-label="`上传 ${displayMetric(metrics?.uploadRate)}`"
    >
      <svg
        class="traffic-metric__arrow"
        data-direction="up"
        viewBox="0 0 24 24"
        fill="none"
        aria-hidden="true"
      >
        <!-- 与 C++ WebUI 的 Lucide ArrowUp 保持同一几何。 -->
        <path d="m5 12 7-7 7 7" />
        <path d="M12 19V5" />
      </svg>
      <span v-if="showTotals" class="traffic-metric__label">上传</span>
      <div class="traffic-metric__values">
        <strong class="traffic-metric__rate" :title="displayMetric(metrics?.uploadRate)" tabindex="0">{{ displayMetric(metrics?.uploadRate) }}</strong>
        <small v-if="showTotals" class="traffic-metric__total" :title="displayMetric(metrics?.uploadTotal)" tabindex="0">{{ displayMetric(metrics?.uploadTotal) }}</small>
      </div>
    </div>

    <div
      class="traffic-metric traffic-metric--download"
      :class="{ 'traffic-metric--with-totals': showTotals }"
      data-testid="traffic-metric-download"
      :aria-label="`下载 ${displayMetric(metrics?.downloadRate)}`"
    >
      <svg
        class="traffic-metric__arrow"
        data-direction="down"
        viewBox="0 0 24 24"
        fill="none"
        aria-hidden="true"
      >
        <!-- 与 C++ WebUI 的 Lucide ArrowDown 保持同一几何。 -->
        <path d="M12 5v14" />
        <path d="m19 12-7 7-7-7" />
      </svg>
      <span v-if="showTotals" class="traffic-metric__label">下载</span>
      <div class="traffic-metric__values">
        <strong class="traffic-metric__rate" :title="displayMetric(metrics?.downloadRate)" tabindex="0">{{ displayMetric(metrics?.downloadRate) }}</strong>
        <small v-if="showTotals" class="traffic-metric__total" :title="displayMetric(metrics?.downloadTotal)" tabindex="0">{{ displayMetric(metrics?.downloadTotal) }}</small>
      </div>
    </div>
  </div>
</template>

<style scoped src="../../../../../../../common/frontend/styles/traffic-metrics.css"></style>
