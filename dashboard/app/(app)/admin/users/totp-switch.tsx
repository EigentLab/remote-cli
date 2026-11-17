"use client";
import { useState, useTransition } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { newTotp, turnOffTotp, turnOnTotp, type TotpEnrolment } from "../actions";
import { Form } from "../form";

/** The signed-in User's own TOTP: off by default; on = scan the QR code, then confirm one code. */
export function TotpSwitch({ on }: { on: boolean }) {
  const [enrolment, setEnrolment] = useState<TotpEnrolment>();
  const [pending, start] = useTransition();

  if (on) {
    return (
      <Form action={turnOffTotp} confirm="Tắt TOTP? Đăng nhập chỉ cần tên và mật khẩu.">
        <span className="text-sm">TOTP đang bật.</span>
        <Input
          name="code"
          aria-label="Mã TOTP hiện tại"
          inputMode="numeric"
          autoComplete="one-time-code"
          pattern="[0-9]{6}"
          maxLength={6}
          required
          className="h-8 w-28"
        />
        <Button size="xs" variant="outline">
          Tắt TOTP
        </Button>
      </Form>
    );
  }
  if (!enrolment) {
    return (
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-sm text-muted-foreground">TOTP đang tắt: đăng nhập chỉ cần tên và mật khẩu.</span>
        <Button size="xs" variant="outline" disabled={pending} onClick={() => start(async () => setEnrolment(await newTotp()))}>
          Bật TOTP
        </Button>
      </div>
    );
  }
  return (
    <div className="grid gap-2">
      <p className="text-sm">Quét mã QR bằng ứng dụng xác thực, rồi nhập mã 6 số để bật.</p>
      {/* eslint-disable-next-line @next/next/no-img-element -- a data: URL, nothing to optimise */}
      <img src={enrolment.qr} alt={`Mã QR TOTP: ${enrolment.url}`} width={180} height={180} />
      <code className="text-xs break-all">{enrolment.secret}</code>
      <Form action={turnOnTotp}>
        <input type="hidden" name="secret" value={enrolment.secret} />
        <Input
          name="code"
          aria-label="Mã TOTP"
          inputMode="numeric"
          autoComplete="one-time-code"
          pattern="[0-9]{6}"
          maxLength={6}
          required
          className="h-8 w-28"
        />
        <Button size="xs">Xác nhận</Button>
        <Button type="button" size="xs" variant="ghost" onClick={() => setEnrolment(undefined)}>
          Huỷ
        </Button>
      </Form>
    </div>
  );
}
