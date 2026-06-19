// Ordo studio — shared UI primitives.
//
// Visual language: solid dark cards on a near-black slate, a single
// lamp-gold accent for primary actions (Ordo brand), "DEFAULT" badge
// vocabulary, copy-ready commands, modal configure dialogs over inline
// expansion, and a quick-template grid for picking from a known set.
//
// (The BrowserOS-derived orange #ff8a3d was retired in favor of the
// Ordo lamp gold #f4c95d so primary actions tie to the lamp lineage
// instead of the borrowed BrowserOS palette.)
//
// Every component is presentational — no fetching, no global state.
// Surfaces compose these against the api.ts client.

import { useEffect, useRef, useState } from "react";
import type { CSSProperties, ReactNode } from "react";
import { motion, AnimatePresence } from "framer-motion";
import { Check, Copy, X, ChevronDown } from "lucide-react";

// ─── tokens ─────────────────────────────────────────────────────

export const FRAUNCES = "'Fraunces', 'Iowan Old Style', Georgia, serif";
export const MONO = "'JetBrains Mono', 'SF Mono', Menlo, monospace";

export const COLORS = {
  ink: "var(--ordo-control-ink, #0a0c10)",
  ink2: "var(--ordo-ink-2, #0e1117)",
  parchment: "var(--ordo-parchment, #f4ecd8)",
  // primary action color — Ordo lamp gold. Used for buttons,
  // highlights, the DEFAULT badge, focused input borders, and the
  // template-card USE affordance.
  primary: "#f4c95d",
  primarySoft: "var(--ordo-primary-soft, #f4c95d22)",
  primarySoftHover: "var(--ordo-primary-soft-hover, #f4c95d33)",
  primaryBorder: "var(--ordo-primary-border, #f4c95d55)",
  // alias kept so existing references to `lamp` (logo, softer accents)
  // continue to resolve to the same gold.
  lamp: "#f4c95d",
  jade: "#7fd1c5",
  violet: "#a99af0",
  rose: "#f07f9f",
  peach: "#f0b67f",
  slate: "#9aa4b2",
  red: "#e85d5d",
  amber: "#f4a13d",
  // surfaces
  cardBg: "var(--ordo-card-bg, #13161c)",
  cardBgRaised: "var(--ordo-card-bg-raised, #181c24)",
  cardBorder: "var(--ordo-card-border, rgba(255,255,255,0.06))",
  cardBorderStrong: "var(--ordo-card-border-strong, rgba(255,255,255,0.1))",
  inputBg: "var(--ordo-input-bg, rgba(0,0,0,0.35))",
  inputBorder: "var(--ordo-input-border, rgba(255,255,255,0.08))",
  inputBorderFocus: "var(--ordo-primary-border, #f4c95d55)",
  textMuted: "var(--ordo-text-muted, rgba(244,236,216,0.6))",
  textDim: "var(--ordo-text-dim, rgba(244,236,216,0.4))",
};

const C = COLORS;

// ─── typography ──────────────────────────────────────────────────

export const Mono = ({
  children,
  size = 11,
  color = C.textMuted,
  upper = false,
  track = 0,
  weight = 400,
  style = {},
}: {
  children: ReactNode;
  size?: number;
  color?: string;
  upper?: boolean;
  track?: number | string;
  weight?: number;
  style?: CSSProperties;
}) => (
  <span
    style={{
      fontFamily: MONO,
      fontSize: size,
      color,
      fontWeight: weight,
      textTransform: upper ? "uppercase" : "none",
      letterSpacing: track,
      ...style,
    }}
  >
    {children}
  </span>
);

export const Serif = ({
  children,
  size = 14,
  color = C.parchment,
  italic = false,
  weight = 400,
  style = {},
}: {
  children: ReactNode;
  size?: number;
  color?: string;
  italic?: boolean;
  weight?: number;
  style?: CSSProperties;
}) => (
  <span
    style={{
      fontFamily: FRAUNCES,
      fontSize: size,
      color,
      fontStyle: italic ? "italic" : "normal",
      fontWeight: weight,
      ...style,
    }}
  >
    {children}
  </span>
);

// ─── card ────────────────────────────────────────────────────────

export const Card = ({
  children,
  className = "",
  style = {},
  padded = true,
}: {
  children: ReactNode;
  className?: string;
  style?: CSSProperties;
  padded?: boolean;
}) => (
  <div
    className={`rounded-xl ${className}`}
    style={{
      background: C.cardBg,
      border: `1px solid ${C.cardBorder}`,
      padding: padded ? 20 : 0,
      ...style,
    }}
  >
    {children}
  </div>
);

// ─── section header card ────────────────────────────────────────

export const SectionHeader = ({
  icon,
  title,
  sub,
  trailing,
}: {
  icon: ReactNode;
  title: string;
  sub?: string;
  trailing?: ReactNode;
}) => (
  <Card padded={false} style={{ padding: "20px 22px" }}>
    <div className="flex items-center gap-4">
      <div
        className="rounded-lg flex items-center justify-center flex-shrink-0"
        style={{
          width: 44,
          height: 44,
          background: C.primarySoft,
          color: C.primary,
        }}
      >
        {icon}
      </div>
      <div style={{ flex: 1, minWidth: 0 }}>
        <div
          style={{
            fontFamily: FRAUNCES,
            fontSize: 22,
            fontWeight: 600,
            color: C.parchment,
            letterSpacing: "-0.01em",
            lineHeight: 1.15,
          }}
        >
          {title}
        </div>
        {sub && (
          <div style={{ marginTop: 3 }}>
            <Mono size={12} color={C.textMuted}>
              {sub}
            </Mono>
          </div>
        )}
      </div>
      {trailing}
    </div>
  </Card>
);

// ─── badge ───────────────────────────────────────────────────────

export const Badge = ({
  children,
  variant = "neutral",
}: {
  children: ReactNode;
  variant?: "primary" | "neutral" | "success" | "warn" | "danger" | "info";
}) => {
  const palette: Record<string, { bg: string; fg: string }> = {
    primary: { bg: C.primary, fg: C.ink },
    neutral: { bg: "rgba(255,255,255,0.06)", fg: C.textMuted },
    success: { bg: `${C.jade}1f`, fg: C.jade },
    warn: { bg: `${C.amber}1f`, fg: C.amber },
    danger: { bg: `${C.red}1f`, fg: C.red },
    info: { bg: `${C.violet}1f`, fg: C.violet },
  };
  const p = palette[variant];
  return (
    <span
      style={{
        fontFamily: MONO,
        fontSize: 9,
        fontWeight: 600,
        textTransform: "uppercase",
        letterSpacing: "0.12em",
        padding: "3px 8px",
        borderRadius: 999,
        background: p.bg,
        color: p.fg,
      }}
    >
      {children}
    </span>
  );
};

// ─── status dot ──────────────────────────────────────────────────

export const Dot = ({
  color = C.jade,
  size = 6,
  glow = true,
}: {
  color?: string;
  size?: number;
  glow?: boolean;
}) => (
  <span
    style={{
      display: "inline-block",
      width: size,
      height: size,
      borderRadius: size,
      background: color,
      boxShadow: glow ? `0 0 ${size}px ${color}` : "none",
      flexShrink: 0,
    }}
  />
);

// ─── buttons ─────────────────────────────────────────────────────

type ButtonVariant = "primary" | "secondary" | "ghost" | "danger";

export const Button = ({
  children,
  onClick,
  disabled = false,
  variant = "secondary",
  size = "md",
  type = "button",
  style = {},
  title,
}: {
  children: ReactNode;
  onClick?: (e: React.MouseEvent<HTMLButtonElement>) => void;
  disabled?: boolean;
  variant?: ButtonVariant;
  size?: "sm" | "md";
  type?: "button" | "submit";
  style?: CSSProperties;
  title?: string;
}) => {
  const padding = size === "sm" ? "5px 10px" : "8px 14px";
  const fontSize = size === "sm" ? 11 : 12;
  const variants: Record<ButtonVariant, CSSProperties> = {
    primary: {
      background: disabled ? "rgba(255,255,255,0.05)" : C.primary,
      color: disabled ? C.slate : C.ink,
      border: "1px solid transparent",
      fontWeight: 600,
    },
    secondary: {
      background: "rgba(255,255,255,0.04)",
      color: C.parchment,
      border: `1px solid ${C.cardBorderStrong}`,
    },
    ghost: {
      background: "transparent",
      color: C.parchment,
      border: "1px solid transparent",
    },
    danger: {
      background: `${C.red}10`,
      color: C.red,
      border: `1px solid ${C.red}33`,
    },
  };
  return (
    <button
      type={type}
      onClick={onClick}
      disabled={disabled}
      title={title}
      style={{
        padding,
        borderRadius: 6,
        fontFamily: MONO,
        fontSize,
        letterSpacing: "0.02em",
        cursor: disabled ? "not-allowed" : "pointer",
        opacity: disabled ? 0.6 : 1,
        transition: "all 0.15s",
        ...variants[variant],
        ...style,
      }}
    >
      {children}
    </button>
  );
};

// ─── form fields ─────────────────────────────────────────────────

export const Field = ({
  label,
  hint,
  required = false,
  children,
}: {
  label: string;
  hint?: ReactNode;
  required?: boolean;
  children: ReactNode;
}) => (
  <div>
    <label
      style={{
        display: "block",
        fontFamily: FRAUNCES,
        fontSize: 13,
        color: C.parchment,
        fontWeight: 500,
        marginBottom: 6,
      }}
    >
      {label}
      {required && <span style={{ color: C.primary }}> *</span>}
    </label>
    {children}
    {hint && (
      <div style={{ marginTop: 5 }}>
        <Mono size={10} color={C.textDim}>
          {hint}
        </Mono>
      </div>
    )}
  </div>
);

const baseInputStyle: CSSProperties = {
  boxSizing: "border-box",
  width: "100%",
  minHeight: 38,
  padding: "9px 12px",
  borderRadius: 6,
  background: C.inputBg,
  border: `1px solid ${C.inputBorder}`,
  color: C.parchment,
  fontFamily: MONO,
  fontSize: 13,
  lineHeight: 1.25,
  outline: "none",
};

export const TextInput = ({
  value,
  onChange,
  placeholder,
  type = "text",
  disabled = false,
  autoFocus = false,
}: {
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  type?: "text" | "password" | "url" | "email";
  disabled?: boolean;
  autoFocus?: boolean;
}) => (
  <input
    type={type}
    value={value}
    onChange={(e) => onChange(e.target.value)}
    placeholder={placeholder}
    disabled={disabled}
    autoFocus={autoFocus}
    style={{ ...baseInputStyle, opacity: disabled ? 0.6 : 1 }}
    onFocus={(e) => (e.currentTarget.style.borderColor = C.inputBorderFocus)}
    onBlur={(e) => (e.currentTarget.style.borderColor = C.inputBorder)}
  />
);

export const NumberInput = ({
  value,
  onChange,
  min,
  max,
  step,
  placeholder,
}: {
  value: number;
  onChange: (v: number) => void;
  min?: number;
  max?: number;
  step?: number;
  placeholder?: string;
}) => (
  <input
    type="number"
    value={value}
    onChange={(e) => onChange(Number(e.target.value))}
    placeholder={placeholder}
    min={min}
    max={max}
    step={step}
    style={baseInputStyle}
    onFocus={(e) => (e.currentTarget.style.borderColor = C.inputBorderFocus)}
    onBlur={(e) => (e.currentTarget.style.borderColor = C.inputBorder)}
  />
);

export const Select = <T extends string>({
  value,
  onChange,
  options,
}: {
  value: T;
  onChange: (v: T) => void;
  options: { value: T; label: string }[];
}) => (
  <div style={{ position: "relative" }}>
    <select
      value={value}
      onChange={(e) => onChange(e.target.value as T)}
      style={{
        ...baseInputStyle,
        appearance: "none",
        WebkitAppearance: "none",
        MozAppearance: "none",
        paddingRight: 32,
        cursor: "pointer",
      }}
    >
      {options.map((o) => (
        <option key={o.value} value={o.value} style={{ background: C.cardBg }}>
          {o.label}
        </option>
      ))}
    </select>
    <ChevronDown
      size={14}
      color={C.textMuted}
      style={{
        position: "absolute",
        right: 10,
        top: "50%",
        transform: "translateY(-50%)",
        pointerEvents: "none",
      }}
    />
  </div>
);

export const Checkbox = ({
  checked,
  onChange,
  label,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label: string;
}) => (
  <label
    style={{
      display: "inline-flex",
      alignItems: "center",
      gap: 8,
      cursor: "pointer",
      userSelect: "none",
    }}
  >
    <span
      style={{
        width: 16,
        height: 16,
        borderRadius: 4,
        background: checked ? C.primary : "rgba(0,0,0,0.35)",
        border: `1px solid ${checked ? C.primary : C.inputBorder}`,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        transition: "all 0.15s",
      }}
    >
      {checked && <Check size={11} color={C.ink} strokeWidth={3} />}
    </span>
    <input
      type="checkbox"
      checked={checked}
      onChange={(e) => onChange(e.target.checked)}
      style={{ display: "none" }}
    />
    <Serif size={13} color={C.parchment}>
      {label}
    </Serif>
  </label>
);

export const Textarea = ({
  value,
  onChange,
  rows = 6,
  placeholder,
  spellCheck = false,
}: {
  value: string;
  onChange: (v: string) => void;
  rows?: number;
  placeholder?: string;
  spellCheck?: boolean;
}) => (
  <textarea
    value={value}
    onChange={(e) => onChange(e.target.value)}
    rows={rows}
    spellCheck={spellCheck}
    placeholder={placeholder}
    style={{
      ...baseInputStyle,
      resize: "vertical",
      minHeight: rows * 18,
      lineHeight: 1.5,
    }}
    onFocus={(e) => (e.currentTarget.style.borderColor = C.inputBorderFocus)}
    onBlur={(e) => (e.currentTarget.style.borderColor = C.inputBorder)}
  />
);

// ─── modal ───────────────────────────────────────────────────────

export const Modal = ({
  open,
  onClose,
  title,
  sub,
  children,
  footer,
  width = 560,
}: {
  open: boolean;
  onClose: () => void;
  title: string;
  sub?: string;
  children: ReactNode;
  footer?: ReactNode;
  width?: number;
}) => {
  // Lock background scroll while the modal is open.
  useEffect(() => {
    if (!open) return;
    const original = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => {
      document.body.style.overflow = original;
      window.removeEventListener("keydown", onKey);
    };
  }, [open, onClose]);
  return (
    <AnimatePresence>
      {open && (
        <motion.div
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: 0.15 }}
          style={{
            position: "fixed",
            inset: 0,
            zIndex: 100,
            background: "rgba(0,0,0,0.6)",
            backdropFilter: "blur(6px)",
            WebkitBackdropFilter: "blur(6px)",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            padding: 20,
          }}
          onClick={onClose}
        >
          <motion.div
            initial={{ scale: 0.96, y: 8, opacity: 0 }}
            animate={{ scale: 1, y: 0, opacity: 1 }}
            exit={{ scale: 0.96, y: 8, opacity: 0 }}
            transition={{ duration: 0.18, ease: "easeOut" }}
            onClick={(e) => e.stopPropagation()}
            style={{
              width,
              maxWidth: "100%",
              maxHeight: "calc(100vh - 40px)",
              background: C.cardBgRaised,
              border: `1px solid ${C.cardBorderStrong}`,
              borderRadius: 12,
              boxShadow: "0 30px 60px -10px rgba(0,0,0,0.6)",
              display: "flex",
              flexDirection: "column",
              overflow: "hidden",
            }}
          >
            <div
              className="flex items-start justify-between"
              style={{ padding: "18px 22px 14px", borderBottom: `1px solid ${C.cardBorder}` }}
            >
              <div style={{ flex: 1, minWidth: 0 }}>
                <div
                  style={{
                    fontFamily: FRAUNCES,
                    fontSize: 18,
                    fontWeight: 600,
                    color: C.parchment,
                  }}
                >
                  {title}
                </div>
                {sub && (
                  <div style={{ marginTop: 3 }}>
                    <Mono size={11} color={C.textMuted}>
                      {sub}
                    </Mono>
                  </div>
                )}
              </div>
              <button
                onClick={onClose}
                style={{
                  background: "transparent",
                  border: "none",
                  color: C.textMuted,
                  cursor: "pointer",
                  padding: 4,
                }}
                title="close"
              >
                <X size={18} />
              </button>
            </div>
            <div style={{ flex: 1, overflow: "auto", padding: "20px 22px" }}>{children}</div>
            {footer && (
              <div
                style={{
                  padding: "14px 22px",
                  borderTop: `1px solid ${C.cardBorder}`,
                  display: "flex",
                  justifyContent: "flex-end",
                  gap: 8,
                }}
              >
                {footer}
              </div>
            )}
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
};

// ─── tab pills (segmented control) ──────────────────────────────

export const TabPills = <T extends string>({
  items,
  active,
  onChange,
}: {
  items: { id: T; label: string }[];
  active: T;
  onChange: (id: T) => void;
}) => (
  <div
    style={{
      display: "inline-flex",
      gap: 2,
      padding: 3,
      background: "rgba(0,0,0,0.3)",
      borderRadius: 8,
      border: `1px solid ${C.cardBorder}`,
    }}
  >
    {items.map((it) => {
      const on = it.id === active;
      return (
        <button
          key={it.id}
          onClick={() => onChange(it.id)}
          style={{
            padding: "5px 12px",
            borderRadius: 6,
            border: "none",
            background: on ? "rgba(255,255,255,0.08)" : "transparent",
            color: on ? C.parchment : C.textMuted,
            fontFamily: FRAUNCES,
            fontSize: 13,
            fontWeight: on ? 500 : 400,
            cursor: "pointer",
            transition: "all 0.15s",
          }}
        >
          {it.label}
        </button>
      );
    })}
  </div>
);

// ─── copyable inline value ──────────────────────────────────────

export const CopyableField = ({
  value,
  label,
}: {
  value: string;
  label?: string;
}) => {
  const [copied, setCopied] = useState(false);
  const onCopy = async () => {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(true);
      setTimeout(() => setCopied(false), 1400);
    } catch {
      // ignore
    }
  };
  return (
    <div>
      {label && (
        <Mono size={11} upper track="0.18em" color={C.textMuted}>
          {label}
        </Mono>
      )}
      <div
        style={{
          marginTop: label ? 8 : 0,
          display: "flex",
          alignItems: "center",
          gap: 8,
          background: C.inputBg,
          border: `1px solid ${C.inputBorder}`,
          borderRadius: 6,
          padding: "8px 10px",
        }}
      >
        <code
          style={{
            flex: 1,
            fontFamily: MONO,
            fontSize: 12,
            color: C.parchment,
            overflow: "hidden",
            textOverflow: "ellipsis",
            whiteSpace: "nowrap",
          }}
        >
          {value}
        </code>
        <Button onClick={onCopy} size="sm" variant="ghost" title="copy">
          {copied ? <Check size={13} color={C.jade} /> : <Copy size={13} />}
        </Button>
      </div>
    </div>
  );
};

// ─── command block (terminal-style) ─────────────────────────────

export const CommandBlock = ({ command }: { command: string }) => {
  const [copied, setCopied] = useState(false);
  const onCopy = async () => {
    try {
      await navigator.clipboard.writeText(command);
      setCopied(true);
      setTimeout(() => setCopied(false), 1400);
    } catch {
      // ignore
    }
  };
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: 10,
        background: C.inputBg,
        border: `1px solid ${C.inputBorder}`,
        borderRadius: 8,
        padding: "12px 14px",
      }}
    >
      <span
        style={{
          fontFamily: MONO,
          fontSize: 12,
          color: C.primary,
          flexShrink: 0,
          fontWeight: 600,
        }}
      >
        $
      </span>
      <code
        style={{
          flex: 1,
          fontFamily: MONO,
          fontSize: 12,
          color: C.parchment,
          overflow: "auto",
          whiteSpace: "nowrap",
        }}
      >
        {command}
      </code>
      <Button onClick={onCopy} size="sm" variant="ghost" title="copy">
        {copied ? <Check size={13} color={C.jade} /> : <Copy size={13} />}
      </Button>
    </div>
  );
};

// ─── template card (provider quick-pick) ────────────────────────

export const TemplateCard = ({
  icon,
  label,
  badge,
  onUse,
  disabled = false,
}: {
  icon: ReactNode;
  label: string;
  badge?: string;
  onUse: () => void;
  disabled?: boolean;
}) => (
  <div
    style={{
      position: "relative",
      background: C.cardBgRaised,
      border: `1px solid ${C.cardBorder}`,
      borderRadius: 10,
      padding: "12px 14px",
      display: "flex",
      alignItems: "center",
      gap: 10,
      transition: "all 0.15s",
    }}
  >
    {badge && (
      <span
        style={{
          position: "absolute",
          top: -8,
          left: 10,
          fontFamily: MONO,
          fontSize: 9,
          fontWeight: 700,
          letterSpacing: "0.15em",
          padding: "2px 7px",
          borderRadius: 4,
          background: C.primary,
          color: C.ink,
        }}
      >
        {badge}
      </span>
    )}
    <div
      style={{
        width: 26,
        height: 26,
        borderRadius: 6,
        background: "rgba(255,255,255,0.05)",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        color: C.parchment,
        flexShrink: 0,
      }}
    >
      {icon}
    </div>
    <span
      style={{
        flex: 1,
        fontFamily: FRAUNCES,
        fontSize: 13,
        color: C.parchment,
        fontWeight: 500,
        minWidth: 0,
        overflow: "hidden",
        textOverflow: "ellipsis",
        whiteSpace: "nowrap",
      }}
    >
      {label}
    </span>
    <Button onClick={onUse} disabled={disabled} variant="ghost" size="sm">
      <span style={{ color: C.primary, fontWeight: 600 }}>USE</span>
    </Button>
  </div>
);

// ─── configured-row (provider/peer/server entry) ────────────────

export const ConfiguredRow = ({
  selected = false,
  icon,
  name,
  defaultBadge = false,
  subtitle,
  rightBadge,
  actions,
  onSelect,
}: {
  selected?: boolean;
  icon: ReactNode;
  name: string;
  defaultBadge?: boolean;
  subtitle?: ReactNode;
  rightBadge?: ReactNode;
  actions?: ReactNode;
  onSelect?: () => void;
}) => (
  <div
    style={{
      background: C.cardBg,
      border: `1px solid ${selected ? C.primaryBorder : C.cardBorder}`,
      borderRadius: 10,
      padding: "12px 14px",
      display: "flex",
      alignItems: "center",
      gap: 12,
      cursor: onSelect ? "pointer" : "default",
      transition: "all 0.15s",
    }}
    onClick={onSelect}
  >
    {onSelect !== undefined && (
      <span
        style={{
          width: 18,
          height: 18,
          borderRadius: 999,
          border: `2px solid ${selected ? C.primary : C.cardBorderStrong}`,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          flexShrink: 0,
        }}
      >
        {selected && (
          <span
            style={{
              width: 8,
              height: 8,
              borderRadius: 999,
              background: C.primary,
            }}
          />
        )}
      </span>
    )}
    <div
      style={{
        width: 30,
        height: 30,
        borderRadius: 6,
        background: "rgba(255,255,255,0.04)",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        color: C.primary,
        flexShrink: 0,
      }}
    >
      {icon}
    </div>
    <div style={{ flex: 1, minWidth: 0 }}>
      <div className="flex items-center gap-2">
        <span style={{ fontFamily: FRAUNCES, fontSize: 14, fontWeight: 600, color: C.parchment }}>
          {name}
        </span>
        {defaultBadge && <Badge variant="primary">DEFAULT</Badge>}
        {rightBadge}
      </div>
      {subtitle && (
        <div style={{ marginTop: 3 }}>
          <Mono size={11} color={C.textMuted}>
            {subtitle}
          </Mono>
        </div>
      )}
    </div>
    {actions && (
      <div
        className="flex items-center gap-2 flex-shrink-0"
        onClick={(e) => e.stopPropagation()}
      >
        {actions}
      </div>
    )}
  </div>
);

// ─── tool card (inventory grid) ─────────────────────────────────

export const ToolCard = ({
  icon,
  name,
  description,
  onClick,
}: {
  icon: ReactNode;
  name: string;
  description: string;
  onClick?: () => void;
}) => (
  <button
    onClick={onClick}
    style={{
      textAlign: "left",
      background: C.cardBgRaised,
      border: `1px solid ${C.cardBorder}`,
      borderRadius: 10,
      padding: "14px 16px",
      cursor: onClick ? "pointer" : "default",
      transition: "all 0.15s",
      display: "flex",
      flexDirection: "column",
      gap: 6,
      width: "100%",
    }}
    onMouseEnter={(e) => {
      e.currentTarget.style.borderColor = C.primaryBorder;
    }}
    onMouseLeave={(e) => {
      e.currentTarget.style.borderColor = C.cardBorder;
    }}
  >
    <div className="flex items-center gap-2">
      <span style={{ color: C.primary, display: "inline-flex" }}>{icon}</span>
      <span
        style={{
          fontFamily: MONO,
          fontSize: 12,
          fontWeight: 600,
          color: C.parchment,
          overflow: "hidden",
          textOverflow: "ellipsis",
          whiteSpace: "nowrap",
        }}
      >
        {name}
      </span>
    </div>
    <span
      style={{
        fontFamily: FRAUNCES,
        fontSize: 12,
        color: C.textMuted,
        lineHeight: 1.4,
        overflow: "hidden",
        display: "-webkit-box",
        WebkitLineClamp: 2,
        WebkitBoxOrient: "vertical",
      }}
    >
      {description}
    </span>
  </button>
);

// ─── toast / inline alert ───────────────────────────────────────

export const Alert = ({
  variant = "info",
  children,
}: {
  variant?: "success" | "warn" | "danger" | "info";
  children: ReactNode;
}) => {
  const palette = {
    success: { bg: `${C.jade}10`, border: `${C.jade}33`, fg: C.jade },
    warn: { bg: `${C.amber}10`, border: `${C.amber}33`, fg: C.amber },
    danger: { bg: `${C.red}10`, border: `${C.red}33`, fg: C.red },
    info: { bg: `${C.violet}10`, border: `${C.violet}33`, fg: C.violet },
  }[variant];
  return (
    <div
      style={{
        background: palette.bg,
        border: `1px solid ${palette.border}`,
        borderRadius: 8,
        padding: "10px 14px",
      }}
    >
      <Mono size={11} color={palette.fg}>
        {children}
      </Mono>
    </div>
  );
};

// ─── keep a ref to the latest value (used by polling effects) ───

export const useLatest = <T,>(value: T) => {
  const ref = useRef(value);
  ref.current = value;
  return ref;
};
