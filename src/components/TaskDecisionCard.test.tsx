import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { TaskDecisionCard } from '$components/TaskDecisionCard';

describe('TaskDecisionCard', () => {
  it('keeps the main conversation as the only answer surface', () => {
    render(<TaskDecisionCard decision={{
      id: 'question-1',
      question: '是否将这次改动限定在当前项目？',
      affects: ['文件范围', '后续构建'],
    }} />);

    expect(screen.getByText('需要你的判断')).toBeInTheDocument();
    expect(screen.getByText('是否将这次改动限定在当前项目？')).toBeInTheDocument();
    expect(screen.getByText('会影响：文件范围、后续构建')).toBeInTheDocument();
    expect(screen.getByText('直接在下方消息中回复即可。')).toBeInTheDocument();
    expect(screen.queryByRole('button')).not.toBeInTheDocument();
  });

  it('does not render when there is no pending decision', () => {
    const { container } = render(<TaskDecisionCard decision={null} />);
    expect(container).toBeEmptyDOMElement();
  });
});
