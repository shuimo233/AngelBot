export const memoryCategoryLabels: Readonly<Record<string, string>> = {
  preference: '偏好', fact: '事实', event: '事项', knowledge: '参考资料', personality: '相处方式',
};

export const memoryCategoryLabel = (category: string) => Object.prototype.hasOwnProperty.call(memoryCategoryLabels, category)
  ? memoryCategoryLabels[category] : category || '未分类';
export const memoryScopeLabel = (scope: string) => scope === 'global' ? '所有对话' : '限定范围';
