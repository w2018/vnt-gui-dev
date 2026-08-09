// 延时分级颜色（连接页 + 设备列表共用）

/** 延时等级 */
export function latencyColor(ms: number | null | undefined): string {
  if (ms == null || ms < 0) return '#f5222d'; // 未检测/超时：红
  if (ms <= 50) return '#52c41a'; // ≤50ms：绿（优）
  if (ms <= 150) return '#fa8c16'; // ≤150ms：橙（中）
  return '#f5222d'; // >150ms：红（差）
}

/** 延时等级文字 */
export function latencyLabel(ms: number | null | undefined): string {
  if (ms == null || ms < 0) return '超时';
  if (ms <= 50) return '优';
  if (ms <= 150) return '中';
  return '差';
}
