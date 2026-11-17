import { api, type User } from "@/lib/api";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import * as act from "../actions";
import { Form } from "../form";
import { card, cardHead, title } from "../../styles";
import { TotpSwitch } from "./totp-switch";

export default async function UsersPage() {
  const [users, totp] = await Promise.all([api<User[]>("/api/v1/users"), api<{ on: boolean }>("/api/v1/me/totp")]);
  return (
    <>
    <section className={`${card} mb-5`} aria-label="TOTP của bạn">
      <div className={cardHead}>
        <h3 className={title}>TOTP của bạn</h3>
      </div>
      <div className="p-4">
        {/* Keyed so turning TOTP off forgets an old enrolment. */}
        <TotpSwitch key={String(totp.on)} on={totp.on} />
      </div>
    </section>
    <section className={card} aria-label="User">
      <div className={cardHead}>
        <h3 className={title}>User</h3>
        <Form action={act.createUser}>
          <Input name="name" aria-label="Tên user mới" placeholder="tên user" required className="h-8 w-40" />
          <Button size="sm" variant="secondary">
            + User
          </Button>
        </Form>
      </div>
      <Table>
        <TableHeader>
          <TableRow className="hover:bg-transparent">
            <TableHead>User</TableHead>
            <TableHead />
          </TableRow>
        </TableHeader>
        <TableBody>
          {users.map((u) => (
            <TableRow key={u.id}>
              <TableCell className="font-extrabold whitespace-normal">
                {u.name}{" "}
                {u.disabled_at && (
                  <Badge className="bg-warn text-ink">
                    đã vô hiệu
                  </Badge>
                )}
              </TableCell>
              <TableCell className="whitespace-normal">
                {!u.disabled_at && (
                  <div className="flex items-center justify-end gap-1.5">
                    <Form action={act.setPassword}>
                      <input type="hidden" name="user" value={u.name} />
                      <Input
                        type="password"
                        name="password"
                        aria-label={`Mật khẩu Dashboard cho ${u.name}`}
                        placeholder="mật khẩu (≥ 8)"
                        autoComplete="new-password"
                        required
                        className="h-8 w-36"
                      />
                      <Button size="xs" variant="outline">
                        Đặt mật khẩu
                      </Button>
                    </Form>
                    <Form action={act.disableUser} confirm={`Vô hiệu ${u.name}? Phiên đăng nhập của họ ngừng hoạt động.`}>
                      <input type="hidden" name="user" value={u.name} />
                      <Button size="xs" variant="ghost" className="text-destructive">
                        Vô hiệu
                      </Button>
                    </Form>
                  </div>
                )}
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </section>
    </>
  );
}
