// 使用逗号作为千位分隔符，便于阅读
export function formatInteger(value: number) {
  return Math.round(value).toString().replace(/\B(?=(\d{3})+(?!\d))/g, ",");
}
