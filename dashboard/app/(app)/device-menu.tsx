"use client";
// A Devices page row's ⋯ menu: rename or remove the Address Book entry (the Server keeps no Device management, ADR 0011).
import { Popover } from "radix-ui";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import type { Device } from "@/lib/api";
import * as act from "./admin/actions";
import { Form } from "./admin/form";

export function DeviceMenu({ device: d }: { device: Device }) {
  const label = `Thao tác cho ${d.name}`;
  return (
    <Popover.Root>
      <Popover.Trigger asChild>
        <Button size="xs" variant="ghost" aria-label={label} className="relative z-10">
          ⋯
        </Button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          aria-label={label}
          align="end"
          className="z-50 grid w-80 gap-3 border-2 border-ink bg-popover p-4 text-sm text-popover-foreground shadow-card"
        >
          <Form action={act.renameInAddressBook}>
            <input type="hidden" name="device" value={d.id} />
            <Input name="name" aria-label="Tên mới" placeholder="tên mới" maxLength={255} required className="h-8 w-36" />
            <Button size="xs" variant="outline">
              Đổi tên
            </Button>
          </Form>
          <Form action={act.removeFromAddressBook} confirm={`Bỏ ${d.name} khỏi Address Book? Device Password đã nhớ sẽ bị xoá.`}>
            <input type="hidden" name="device" value={d.id} />
            <Button size="xs" variant="outline">
              Bỏ khỏi Address Book
            </Button>
          </Form>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
