// 极简主题状态（亮/暗，localStorage 持久化）

const KEY = 'vnt-theme';
let dark = localStorage.getItem(KEY) === 'dark';
const listeners: ((d: boolean) => void)[] = [];

export function isDark(): boolean {
  return dark;
}

export function setDark(v: boolean): void {
  dark = v;
  localStorage.setItem(KEY, v ? 'dark' : 'light');
  listeners.forEach((l) => l(v));
}

export function onThemeChange(l: (d: boolean) => void): () => void {
  listeners.push(l);
  return () => {
    const i = listeners.indexOf(l);
    if (i >= 0) listeners.splice(i, 1);
  };
}
