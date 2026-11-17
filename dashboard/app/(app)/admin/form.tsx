"use client";
import { startTransition, useActionState, useRef, type ReactNode } from "react";
import { cn } from "@/lib/utils";
import type { Result } from "./actions";

/** A form posting to an admin server action; shows its one-time result (new token, code...) or error. */
export function Form({
  action,
  confirm,
  children,
  label,
  className,
}: {
  action: (prev: Result, form: FormData) => Promise<Result>;
  /** Asks before submitting (destructive actions). */
  confirm?: string;
  children: ReactNode;
  label?: string;
  className?: string;
}) {
  const ref = useRef<HTMLFormElement>(null);
  // Cleared only on success: a refused submit keeps what was typed (React's `action={...}` resets either way).
  const [state, run, pending] = useActionState(async (prev: Result, form: FormData) => {
    const result = await action(prev, form);
    if (!result.error) ref.current?.reset();
    return result;
  }, {});
  return (
    <form
      ref={ref}
      aria-label={label}
      className={cn("flex flex-wrap items-center gap-2", className)}
      onSubmit={(e) => {
        e.preventDefault();
        if (confirm && !window.confirm(confirm)) return;
        const form = new FormData(e.currentTarget);
        startTransition(() => run(form));
      }}
    >
      <fieldset disabled={pending} className="contents">
        {children}
      </fieldset>
      {state.message && (
        <output role="status" className="basis-full text-xs font-medium break-all">
          {state.message}
        </output>
      )}
      {state.error && (
        <span role="alert" className="basis-full text-sm text-destructive">
          {state.error}
        </span>
      )}
    </form>
  );
}
