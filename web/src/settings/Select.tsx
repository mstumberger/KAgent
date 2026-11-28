// The themed replacement of a native `<select>` (shell.md section 4, rule 7): the same call, `<option>` children and an
// `onChange` that reads `e.target.value`, drawn by `Dropdown` so the list carries the colours of the page and the
// keyboard behaviour of the house. A `<label htmlFor>` still points at it through `id`.
import { Children, Fragment, isValidElement, type ReactNode } from "react";
import Dropdown, { type DropdownOption } from "./Dropdown";

interface Props {
  value: string | number;
  /** What a `<select>` handler reads: `e.target.value`. */
  onChange?: (e: { target: { value: string } }) => void;
  children: ReactNode;
  disabled?: boolean;
  id?: string;
  className?: string;
  "aria-label"?: string;
}

const text = (node: ReactNode): string => (Array.isArray(node) ? node.map(text).join("") : typeof node === "string" || typeof node === "number" ? String(node) : isValidElement(node) ? text((node.props as { children?: ReactNode }).children) : "");

/** The `<option>`s among `children`, through fragments and arrays. */
function optionsOf(children: ReactNode): DropdownOption[] {
  const out: DropdownOption[] = [];
  for (const child of Children.toArray(children)) {
    if (!isValidElement(child)) continue;
    const props = child.props as { value?: string | number; disabled?: boolean; children?: ReactNode };
    if (child.type === Fragment) out.push(...optionsOf(props.children));
    else if (child.type === "option") out.push({ value: String(props.value ?? text(props.children)), label: props.children, disabled: props.disabled });
  }
  return out;
}

export default function Select({ value, onChange, children, disabled, id, className, "aria-label": ariaLabel }: Props) {
  return <Dropdown id={id} ariaLabel={ariaLabel} className={className} disabled={disabled} value={String(value)} options={optionsOf(children)} onChange={(v) => onChange?.({ target: { value: v } })} />;
}
