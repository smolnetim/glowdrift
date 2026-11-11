import { useEffect, useId, useState } from "react";

interface PercentFieldProps {
  label: string;
  /** Fraction 0..1. */
  value: number;
  onCommit: (fraction: number) => void;
}

const toText = (fraction: number): string => String(Math.round(fraction * 1000) / 10);

/** Numeric input in percent that commits valid values as the user types. */
export function PercentField({ label, value, onCommit }: PercentFieldProps) {
  const id = useId();
  const [text, setText] = useState(toText(value));
  const [focused, setFocused] = useState(false);

  useEffect(() => {
    if (!focused) setText(toText(value));
  }, [value, focused]);

  return (
    <div className="percent-field">
      <label htmlFor={id}>{label}</label>
      <div className="percent-input">
        <input
          id={id}
          type="number"
          inputMode="decimal"
          min={0}
          max={100}
          step={1}
          value={text}
          onFocus={() => setFocused(true)}
          onBlur={() => {
            setFocused(false);
            setText(toText(value));
          }}
          onChange={(e) => {
            setText(e.currentTarget.value);
            const parsed = Number.parseFloat(e.currentTarget.value);
            if (Number.isFinite(parsed)) onCommit(parsed / 100);
          }}
        />
        <span aria-hidden="true">%</span>
      </div>
    </div>
  );
}
