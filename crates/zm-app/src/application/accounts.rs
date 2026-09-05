use super::*;

impl ZmApp {
    pub(super) fn add_managed_account(&mut self, ctx: egui::Context) {
        let account_name = self.manager_account.trim().to_owned();
        if account_name.is_empty() || self.manager_password.is_empty() {
            self.status = "请输入新用户的用户名和密码".into();
            return;
        }
        if self
            .config
            .accounts
            .iter()
            .any(|account| account.account == account_name)
        {
            self.status = "该用户已存在，可直接点击切换".into();
            return;
        }

        self.launch.cancel();
        self.captcha_id = None;
        self.captcha_url = None;
        self.captcha_texture = None;
        self.captcha_value.clear();
        let mut account = AccountConfig::new(&account_name);
        account.remember_password = self.manager_save_password;
        let password = self.manager_password.clone();
        let save_to_keyring = self.manager_save_password;
        let previous_last_account = self.config.last_account;
        self.config.accounts.push(account.clone());
        self.config.last_account = Some(account.id);
        if let Err(error) = self.save_config() {
            self.config.accounts.retain(|entry| entry.id != account.id);
            self.config.last_account = previous_last_account;
            self.status = format!("添加用户失败：{error}");
            return;
        }

        let reply = self.credentials.save(
            &account.credential_id,
            &account.account,
            &password,
            save_to_keyring,
        );
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            if let Err(error) = receive_credential(reply).await {
                let _ = tx.send(AppMessage::Notice(format!(
                    "凭据操作失败，系统存储状态未更新：{error}"
                )));
            }
            ctx.request_repaint();
        });

        self.credential_request_id = self.credential_request_id.wrapping_add(1);
        self.account_mode = AccountMode::Saved(account.id);
        self.account = account_name;
        self.password.clone_from(&self.manager_password);
        self.credential_state = CredentialState::Available;
        self.save_password = save_to_keyring;
        self.manager_account.clear();
        self.manager_password.clear();
        self.account_picker_open = false;
        self.status = "新用户已添加并切换".into();
    }

    pub(super) fn delete_managed_account(&mut self, id: Uuid, ctx: egui::Context) {
        let Some(index) = self.config.accounts.iter().position(|entry| entry.id == id) else {
            return;
        };
        let removed = self.config.accounts.remove(index);
        let previous_last_account = self.config.last_account;
        if self.config.last_account == Some(id) {
            self.config.last_account = None;
        }
        if let Err(error) = self.save_config() {
            self.config.accounts.insert(index, removed);
            self.config.last_account = previous_last_account;
            self.status = format!("删除用户失败：{error}");
            return;
        }
        let reply = self
            .credentials
            .delete(&removed.credential_id, &removed.account);
        let repaint_ctx = ctx.clone();
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            if let Err(error) = receive_credential(reply).await {
                let _ = tx.send(AppMessage::Notice(format!(
                    "账号记录已删除，但系统凭据删除失败：{error}"
                )));
            }
            repaint_ctx.request_repaint();
        });
        if self.account_mode == AccountMode::Saved(id) {
            self.select_account(AccountMode::New, ctx);
            self.account_picker_open = true;
        }
        self.status = "用户已删除".into();
    }
}
