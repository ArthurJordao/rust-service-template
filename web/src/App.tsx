import { BrowserRouter, Navigate, Route, Routes } from "react-router-dom";
import { RequireAuth, RequireAdmin } from "@/auth/guards";
import { AppLayout } from "@/components/AppLayout";
import { LoginPage } from "@/routes/LoginPage";
import { RegisterPage } from "@/routes/RegisterPage";
import { AccountPage } from "@/routes/AccountPage";
import { UsersPage } from "@/routes/admin/UsersPage";
import { DlqPage } from "@/routes/admin/DlqPage";

export function App() {
  return (
    <BrowserRouter>
      <Routes>
        <Route path="/login" element={<LoginPage />} />
        <Route path="/register" element={<RegisterPage />} />
        <Route element={<RequireAuth />}>
          <Route element={<AppLayout />}>
            <Route path="/" element={<AccountPage />} />
            <Route element={<RequireAdmin />}>
              <Route path="/admin/users" element={<UsersPage />} />
              <Route path="/admin/dlq" element={<DlqPage />} />
            </Route>
          </Route>
        </Route>
        <Route path="*" element={<Navigate to="/" replace />} />
      </Routes>
    </BrowserRouter>
  );
}
