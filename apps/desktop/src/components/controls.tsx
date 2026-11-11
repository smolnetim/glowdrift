import { type ReactNode, useId } from "react";
import { cssVars } from "../lib/format";

interface ToggleRowProps {
  title: string;
  description?: string;
  checked: boolean;
  disabled?: boolean;
  onChange: (checked: boolean) => void;
}

/** A labelled on/off switch row. */
export function ToggleRow({ title, description, checked, disabled, onChange }: ToggleRowProps) {
  const titleId = useId();
  const descriptionId = useId();
  return (
    <div className="setting-row">
      <div className="setting-text">
        <div className="setting-title" id={titleId}>
          {title}
        </div>
        {description && (
          <div className="setting-description" id={descriptionId}>
            {description}
          </div>
        )}
      </div>
      <button
        type="button"
        role="switch"
        className="switch"
        aria-checked={checked}
        aria-labelledby={titleId}
        aria-describedby={description ? descriptionId : undefined}
        disabled={disabled}
        onClick={() => onChange(!checked)}
      >
        <span className="switch-thumb" />
      </button>
    </div>
  );
}

interface SliderFieldProps {
  label: string;
  description?: string;
  value: number;
  min: number;
  max: number;
  step?: number;
  /** Text shown next to the label and announced by screen readers. */
  valueText: string;
  onChange: (value: number) => void;
  children?: ReactNode;
}

export function SliderField({
  label,
  description,
  value,
  min,
  max,
  step = 1,
  valueText,
  onChange,
  children,
}: SliderFieldProps) {
  const id = useId();
  const descriptionId = useId();
  const fill = ((value - min) / (max - min)) * 100;
  return (
    <div className="slider-field">
      <div className="slider-head">
        <label htmlFor={id} className="setting-title">
          {label}
        </label>
        <output htmlFor={id} className="slider-value">
          {valueText}
        </output>
      </div>
      {description && (
        <p className="setting-description" id={descriptionId}>
          {description}
        </p>
      )}
      <input
        id={id}
        className="slider"
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        aria-valuetext={valueText}
        aria-describedby={description ? descriptionId : undefined}
        style={cssVars({ "--fill": `${fill}%` })}
        onChange={(e) => onChange(Number(e.currentTarget.value))}
      />
      {children}
    </div>
  );
}

export interface ChoiceOption<T extends string> {
  value: T;
  title: string;
  description?: string;
}

interface ChoiceGroupProps<T extends string> {
  legend: string;
  name: string;
  value: T | null;
  options: ChoiceOption<T>[];
  onChange: (value: T) => void;
  columns?: "auto" | "one";
}

/** Radio cards; native radio inputs provide the arrow-key behaviour. */
export function ChoiceGroup<T extends string>({
  legend,
  name,
  value,
  options,
  onChange,
  columns = "auto",
}: ChoiceGroupProps<T>) {
  return (
    <fieldset className="choice-group">
      <legend className="visually-hidden">{legend}</legend>
      <div className={`choice-grid choice-grid--${columns}`}>
        {options.map((option) => (
          <label className="choice" key={option.value}>
            <input
              type="radio"
              name={name}
              value={option.value}
              checked={option.value === value}
              onChange={() => onChange(option.value)}
            />
            <span className="choice-body">
              <span className="choice-title">{option.title}</span>
              {option.description && <span className="choice-description">{option.description}</span>}
            </span>
          </label>
        ))}
      </div>
    </fieldset>
  );
}
