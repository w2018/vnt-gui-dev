// 延时分级颜色（连接页 + 设备列表共用）
// 绿 0~60 / 橙 60~120 / 红 >120 / 红 超时·未检测

/** 延时等级颜色 */
export function latencyColor(ms: number | null | undefined): string {
  if (ms == null || ms < 0) return '#f5222d'; // 超时/未检测：红
  if (ms <= 60) return '#52c41a'; // ≤60ms：绿（优）
  if (ms <= 120) return '#fa8c16'; // 60~120ms：橙（中）
  return '#f5222d'; // >120ms：红（差）
}

/** 延时等级文字 */
export function latencyLabel(ms: number | null | undefined): string {
  if (ms == null || ms < 0) return '超时';
  if (ms <= 60) return '优';
  if (ms <= 120) return '中';
  return '差';
}
