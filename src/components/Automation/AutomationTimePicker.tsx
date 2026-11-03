import { useEffect, useId, useRef, useState } from 'react';
import { Button, Dialog, IconButton } from '../../ui';

const pad = (number: number) => String(number).padStart(2, '0');
const parseTime = (value: string): [number, number] => /^(?:[01]\d|2[0-3]):[0-5]\d$/.test(value)
  ? [Number(value.slice(0, 2)), Number(value.slice(3))] : [9, 0];

/** Reuses the shared modal/focus boundary; only confirmation changes the form. */
export function AutomationTimePicker({ value, onChange }: { value: string; onChange: (value: string) => void }) {
  const [open, setOpen] = useState(false);
  const [part, setPart] = useState<'hour' | 'minute'>('hour');
  const [[hour, minute], setDraft] = useState(() => parseTime(value));
  const clockRef = useRef<SVGSVGElement>(null);
  const labelId = useId();
  const numbers = part === 'hour' ? Array.from({ length: 24 }, (_, index) => index) : Array.from({ length: 12 }, (_, index) => index * 5);
  const selected = part === 'hour' ? hour : minute;
  const angle = (part === 'hour' ? hour % 12 : minute / 5) * Math.PI / 6;
  const radius = part === 'hour' && (hour === 0 || hour > 12) ? 28 : 42;

  useEffect(() => {
    if (open) clockRef.current?.focus({ preventScroll: true });
  }, [open, part]);

  const choose = (number: number) => {
    setDraft(part === 'hour' ? [number, minute] : [hour, number]);
    if (part === 'hour') setPart('minute');
  };
  const confirm = () => {
    onChange(`${pad(hour)}:${pad(minute)}`);
    setOpen(false);
  };

  return <div className="automation-time-picker">
    <span id={labelId} className="automation-time-label">每天执行时间</span>
    <div className="automation-time-value" role="group" aria-labelledby={labelId}>
      <span>{value || '—'}</span>
      <IconButton label="选择执行时间" aria-haspopup="dialog" aria-expanded={open} onClick={() => {
        setDraft(parseTime(value)); setPart('hour'); setOpen(true);
      }}><svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round"><circle cx="12" cy="12" r="9" /><path d="M12 7v5l3 2" /></svg></IconButton>
    </div>
    <Dialog open={open} onClose={() => setOpen(false)} title="选择执行时间" className="automation-clock-panel" initialFocusSelector="[role='slider']">
        <div className="automation-clock-tabs" role="group" aria-label="选择时间部分">
          <Button variant="ghost" aria-pressed={part === 'hour'} onClick={() => setPart('hour')}>小时 {pad(hour)}</Button>
          <span>:</span>
          <Button variant="ghost" aria-pressed={part === 'minute'} onClick={() => setPart('minute')}>分钟 {pad(minute)}</Button>
        </div>
        <div className="automation-clock" role="group" aria-label={part === 'hour' ? '选择小时' : '选择分钟'}>
          <svg ref={clockRef} viewBox="0 0 100 100" role="slider" tabIndex={0} aria-label={part === 'hour' ? '小时表盘' : '分钟表盘'} aria-valuemin={0} aria-valuemax={part === 'hour' ? 23 : 59} aria-valuenow={selected} aria-valuetext={`${pad(selected)} ${part === 'hour' ? '时' : '分'}`}
            onPointerDown={event => {
              if (event.button !== 0) return;
              const rect = event.currentTarget.getBoundingClientRect();
              const x = event.clientX - rect.left - rect.width / 2;
              const y = event.clientY - rect.top - rect.height / 2;
              if (Math.hypot(x, y) < rect.width * .12) return;
              const fraction = (Math.atan2(x, -y) / (2 * Math.PI) + 1) % 1;
              const tick = Math.round(fraction * (part === 'hour' ? 12 : 60)) % (part === 'hour' ? 12 : 60);
              choose(part === 'minute' ? tick : Math.hypot(x, y) < rect.width * .35 ? tick === 0 ? 0 : tick + 12 : tick === 0 ? 12 : tick);
            }}
            onKeyDown={event => {
              let number = selected;
              const limit = part === 'hour' ? 24 : 60;
              if (event.key === 'ArrowUp' || event.key === 'ArrowRight') number = (selected + 1) % limit;
              else if (event.key === 'ArrowDown' || event.key === 'ArrowLeft') number = (selected + limit - 1) % limit;
              else if (event.key === 'Home') number = 0;
              else if (event.key === 'End') number = limit - 1;
              else if (event.key === 'Enter' || event.key === ' ') {
                event.preventDefault();
                if (part === 'hour') setPart('minute'); else confirm();
                return;
              } else return;
              event.preventDefault();
              setDraft(part === 'hour' ? [number, minute] : [hour, number]);
            }}>
            {part === 'minute' && Array.from({ length: 60 }, (_, number) => <line key={number} x1={50 + Math.sin(number * Math.PI / 30) * 47} y1={50 - Math.cos(number * Math.PI / 30) * 47} x2={50 + Math.sin(number * Math.PI / 30) * (number % 5 ? 45.5 : 44)} y2={50 - Math.cos(number * Math.PI / 30) * (number % 5 ? 45.5 : 44)} />)}
          </svg>
          <span className="automation-clock-hand" aria-hidden="true" style={{ height: `${radius}%`, transform: `rotate(${angle}rad)` }} />
          {numbers.map(number => {
            const tick = part === 'hour' ? number % 12 : number / 5;
            const distance = part === 'hour' && (number === 0 || number > 12) ? 28 : 42;
            return <button key={number} type="button" aria-label={`${pad(number)} ${part === 'hour' ? '时' : '分'}`} aria-pressed={number === selected}
              style={{ left: `${50 + Math.sin(tick * Math.PI / 6) * distance}%`, top: `${50 - Math.cos(tick * Math.PI / 6) * distance}%` }}
              onClick={() => choose(number)}>{pad(number)}</button>;
          })}
        </div>
        <small>{part === 'hour' ? '内圈为 00、13–23 时。' : '方向键可逐分钟调整。'}</small>
        <div className="automation-clock-actions"><Button onClick={() => setOpen(false)}>取消</Button><Button variant="primary" onClick={() => part === 'hour' ? setPart('minute') : confirm()}>{part === 'hour' ? '选择分钟' : '确定'}</Button></div>
    </Dialog>
    <small>按系统时区执行。</small>
  </div>;
}
