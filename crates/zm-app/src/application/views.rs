use super::*;

impl ZmApp {
    pub(super) fn login_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            ui.add(egui::Image::new((
                self.app_icon.id(),
                egui::vec2(42.0, 42.0),
            )));
            ui.vertical(|ui| {
                ui.heading("造梦游戏库");
                ui.label(
                    egui::RichText::new("选择游戏，使用你的 4399 账号开始冒险")
                        .color(palette::MUTED),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("设置与诊断").clicked() {
                    self.page = Page::Settings;
                }
                if ui.button("管理账号").clicked() {
                    self.account_picker_open = true;
                }
            });
        });
        ui.add_space(24.0);
        ui.columns(2, |columns| {
            for (column, game) in columns.iter_mut().zip([GameKind::Zm4, GameKind::Zm5]) {
                let selected = self.selected_game == game;
                egui::Frame::new()
                    .fill(palette::CARD)
                    .corner_radius(14)
                    .stroke(egui::Stroke::new(
                        1.0_f32,
                        if selected {
                            palette::ACCENT
                        } else {
                            palette::BORDER
                        },
                    ))
                    .inner_margin(20)
                    .show(column, |ui| {
                        ui.label(
                            egui::RichText::new(format!("0{}", game.number()))
                                .size(32.0)
                                .color(palette::ACCENT),
                        );
                        ui.heading(game.display_name());
                        ui.label(
                            egui::RichText::new("官方资源 · 独立游戏会话").color(palette::MUTED),
                        );
                        ui.add_space(12.0);
                        if ui
                            .add_sized(
                                [ui.available_width(), 36.0],
                                egui::Button::new(if selected {
                                    "已选择"
                                } else {
                                    "选择游戏"
                                }),
                            )
                            .clicked()
                            && !selected
                        {
                            self.selected_game = game;
                            self.launch.cancel();
                            self.captcha_id = None;
                            self.captcha_url = None;
                            self.captcha_texture = None;
                            self.captcha_value.clear();
                        }
                    });
            }
        });
        ui.add_space(20.0);
        egui::Frame::new()
            .fill(palette::PANEL)
            .corner_radius(14)
            .inner_margin(20)
            .show(ui, |ui| {
                ui.heading(format!("登录 {}", self.selected_game.display_name()));
                ui.add_space(10.0);
                let previous_account = self.account.clone();
                match self.account_mode {
                    AccountMode::Saved(id) => self.saved_account_ui(ui, id),
                    AccountMode::New => self.new_account_ui(ui),
                }
                if previous_account != self.account {
                    self.launch.cancel();
                    self.captcha_id = None;
                    self.captcha_url = None;
                    self.captcha_texture = None;
                    self.captcha_value.clear();
                }
                if self.captcha_id.is_some() {
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.captcha_value)
                                .hint_text("验证码")
                                .desired_width(150.0),
                        );
                        if let Some(texture) = &self.captcha_texture {
                            ui.add(
                                egui::Image::new((texture.id(), texture.size_vec2()))
                                    .max_width(140.0),
                            );
                        } else {
                            ui.label("图片未加载");
                        }
                        if ui.button("刷新验证码").clicked() {
                            self.refresh_captcha(ctx.clone());
                        }
                    });
                }
                ui.checkbox(&mut self.save_password, "记住密码（系统密钥环）");
                ui.add_space(10.0);
                let ready = !matches!(self.credential_state, CredentialState::Loading { .. });
                if ui
                    .add_enabled(
                        ready,
                        egui::Button::new("登录并启动游戏")
                            .fill(palette::PRIMARY)
                            .min_size(egui::vec2(220.0, 42.0)),
                    )
                    .clicked()
                {
                    self.begin_login(self.selected_game, ctx.clone());
                }
            });
    }

    pub(super) fn saved_account_ui(&mut self, ui: &mut egui::Ui, id: Uuid) {
        let Some(saved) = self.config.accounts.iter().find(|account| account.id == id) else {
            return;
        };
        egui::Frame::new()
            .fill(palette::FIELD)
            .stroke(egui::Stroke::new(1.0_f32, palette::BORDER))
            .corner_radius(12)
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    egui::Frame::new()
                        .fill(egui::Color32::from_rgb(38, 54, 77))
                        .corner_radius(10)
                        .inner_margin(egui::Margin::symmetric(12, 8))
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new("孙")
                                    .size(22.0)
                                    .strong()
                                    .color(palette::ACCENT),
                            );
                        });
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new(&saved.display_name).strong());
                        ui.label(
                            egui::RichText::new(&saved.account)
                                .small()
                                .color(palette::MUTED),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        egui::Frame::new()
                            .fill(egui::Color32::from_rgba_unmultiplied(48, 173, 139, 28))
                            .corner_radius(12)
                            .inner_margin(egui::Margin::symmetric(10, 5))
                            .show(ui, |ui| {
                                ui.label(
                                    egui::RichText::new("当前用户")
                                        .small()
                                        .color(palette::SUCCESS),
                                );
                            });
                    });
                });
            });
        match &self.credential_state {
            CredentialState::Loading { .. } => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    if ui.button("取消启动").clicked() {
                        self.stop_game("启动已取消".into(), false);
                    }
                    ui.label("正在从系统密钥环读取密码…");
                });
            }
            CredentialState::Available => {
                ui.label(
                    egui::RichText::new("✓ 已从系统密钥环安全读取密码")
                        .small()
                        .color(palette::SUCCESS),
                );
            }
            CredentialState::Missing => self.password_input(ui, "此账号没有已保存密码"),
            CredentialState::Error(error) => {
                let error = error.clone();
                ui.label(
                    egui::RichText::new(error)
                        .small()
                        .color(palette::ACCENT_HOVER),
                );
                self.password_input(ui, "密钥环不可用，请输入密码");
            }
        }
    }

    pub(super) fn new_account_ui(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("4399账号").small().strong());
        ui.add(
            egui::TextEdit::singleline(&mut self.account)
                .hint_text("请输入4399账号")
                .desired_width(f32::INFINITY),
        );
        self.password_input(ui, "请输入账号密码");
    }

    pub(super) fn password_input(&mut self, ui: &mut egui::Ui, hint: &str) {
        ui.add_space(3.0);
        ui.label(egui::RichText::new("密码").small().strong());
        ui.add(
            egui::TextEdit::singleline(&mut self.password)
                .password(true)
                .hint_text(hint)
                .desired_width(f32::INFINITY),
        );
    }

    pub(super) fn busy_ui(&mut self, ui: &mut egui::Ui) {
        const STEPS: [&str; 4] = ["4399认证", "资源检查", "创建播放器", "注入会话"];
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.16).max(35.0));
            ui.add(
                egui::Image::new((self.app_icon.id(), egui::vec2(78.0, 78.0))).corner_radius(18),
            );
            ui.add_space(16.0);
            ui.heading(format!("正在启动 {}", self.selected_game.display_name()));
            ui.label(egui::RichText::new(&self.status).color(palette::MUTED));
            ui.add_space(22.0);
            ui.horizontal(|ui| {
                for (index, label) in STEPS.iter().enumerate() {
                    let color = if index < self.busy_step {
                        palette::SUCCESS
                    } else if index == self.busy_step {
                        palette::ACCENT
                    } else {
                        palette::MUTED_DARK
                    };
                    egui::Frame::new()
                        .fill(palette::CARD)
                        .stroke(egui::Stroke::new(1.0_f32, color))
                        .corner_radius(12)
                        .inner_margin(egui::Margin::symmetric(18, 12))
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(format!("{}  {label}", index + 1))
                                    .strong()
                                    .color(color),
                            );
                        });
                }
            });
            ui.add_space(18.0);
            ui.spinner();
        });
    }

    pub(super) fn game_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, fullscreen: bool) {
        if !fullscreen {
            egui::Frame::new()
                .fill(palette::PANEL)
                .inner_margin(egui::Margin::symmetric(12, 7))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let account = self
                            .active_account
                            .as_ref()
                            .map(|account| account.display_name.as_str())
                            .unwrap_or("未登录");
                        let game = self
                            .active_game
                            .map(GameKind::display_name)
                            .unwrap_or("游戏");
                        ui.label(
                            egui::RichText::new(format!("{game}  ·  {account}"))
                                .strong()
                                .color(palette::ACCENT_TEXT),
                        );
                        ui.separator();
                        ui.label("音量");
                        if ui
                            .add(
                                egui::Slider::new(&mut self.config.volume, 0.0..=1.0)
                                    .show_value(false)
                                    .max_decimals(2),
                            )
                            .changed()
                        {
                            self.player.set_volume(self.config.volume);
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("退出游戏").clicked() {
                                self.stop_game("游戏已退出".into(), false);
                            }
                            if ui.button("全屏 F11").clicked() {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
                            }
                            if ui.button("诊断").clicked() {
                                self.diagnostics_open = true;
                            }
                            if ui.button("切换账号").clicked() {
                                self.confirm_switch = true;
                            }
                        });
                    });
                });
        }

        let available = ui.available_rect_before_wrap();
        ui.painter()
            .rect_filled(available, 0.0, egui::Color32::BLACK);
        let game_aspect = GAME_WIDTH as f32 / GAME_HEIGHT as f32;
        let available_aspect = available.width() / available.height().max(1.0);
        let size = if available_aspect > game_aspect {
            egui::vec2(available.height() * game_aspect, available.height())
        } else {
            egui::vec2(available.width(), available.width() / game_aspect)
        };
        let game_rect = egui::Rect::from_center_size(available.center(), size);
        let response = ui.allocate_rect(game_rect, egui::Sense::click_and_drag());
        if response.clicked() {
            response.request_focus();
        }
        if let Some(texture_id) = self.player.texture_id() {
            egui::Image::new((texture_id, size)).paint_at(ui, game_rect);
        }

        let events = ctx.input(|input| input.raw.events.clone());
        let next_frame = self.player.tick(GameFrameInput {
            viewport: game_rect,
            events,
            focused: (response.has_focus() || response.hovered())
                && !self.confirm_switch
                && !self.diagnostics_open
                && !self.account_picker_open
                && ctx.memory(|memory| memory.focused().is_none_or(|id| id == response.id)),
        });
        ctx.request_repaint_after(next_frame);
    }

    pub(super) fn settings_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("设置");
        ui.add_space(8.0);
        if ui
            .add(egui::Slider::new(&mut self.config.volume, 0.0..=1.0).text("音量"))
            .changed()
        {
            self.player.set_volume(self.config.volume);
        }
        ui.label(format!("缓存目录：{}", self.paths.cache_dir.display()));
        ui.label(format!("日志目录：{}", self.paths.log_dir.display()));
        ui.label("开发者：gcd-fj");
        ui.add_space(8.0);
        #[cfg(target_os = "linux")]
        ui.horizontal(|ui| {
            if ui.button("重新安装桌面入口").clicked() {
                self.status = match desktop::install() {
                    Ok(path) => format!("桌面入口已安装：{}", path.display()),
                    Err(error) => error,
                };
            }
            if ui.button("卸载桌面入口").clicked() {
                self.status = match desktop::uninstall() {
                    Ok(()) => "桌面入口已卸载（不会删除程序或缓存）".into(),
                    Err(error) => error,
                };
            }
        });
        if ui.button("清空全部游戏缓存").clicked() {
            let assets = self.assets.clone();
            let tx = self.tx.clone();
            let ctx = ui.ctx().clone();
            self.rt.spawn(async move {
                let result = assets.clear_cache(CacheScope::All).await;
                let _ = tx.send(
                    result
                        .map(|_| AppMessage::CacheCleared)
                        .unwrap_or_else(|error| {
                            AppMessage::Notice(format!("清理缓存失败：{error}"))
                        }),
                );
                ctx.request_repaint();
            });
        }
        if let AccountMode::Saved(id) = self.account_mode
            && ui.button("删除当前保存的账号").clicked()
        {
            self.delete_managed_account(id, ui.ctx().clone());
        }
        ui.separator();
        let diagnostics = self.diagnostics();
        egui::ScrollArea::vertical()
            .max_height(160.0)
            .show(ui, |ui| ui.monospace(&diagnostics));
        if ui.button("复制诊断信息").clicked() {
            ui.ctx().copy_text(diagnostics);
            self.status = "诊断信息已复制".into();
        }
        if ui.button("返回登录页").clicked() {
            let _ = self.save_config();
            self.page = Page::Login;
        }
    }

    pub(super) fn account_picker(&mut self, ctx: &egui::Context) {
        if !self.account_picker_open {
            return;
        }
        let mut open = self.account_picker_open;
        let mut selection = None;
        let mut deletion = None;
        let mut add_requested = false;
        egui::Window::new("管理用户")
            .collapsible(false)
            .resizable(false)
            .default_width(620.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(egui::RichText::new("集中管理用于登录4399的账号").color(palette::MUTED));
                ui.add_space(10.0);
                ui.columns(2, |columns| {
                    columns[0].label(egui::RichText::new("已保存用户").strong());
                    columns[0].add_space(6.0);
                    egui::ScrollArea::vertical()
                        .max_height(260.0)
                        .show(&mut columns[0], |ui| {
                            for account in &self.config.accounts {
                                let selected = self.account_mode == AccountMode::Saved(account.id);
                                egui::Frame::new()
                                    .fill(if selected {
                                        egui::Color32::from_rgb(38, 54, 77)
                                    } else {
                                        palette::FIELD
                                    })
                                    .stroke(egui::Stroke::new(
                                        1.0_f32,
                                        if selected {
                                            palette::ACCENT
                                        } else {
                                            palette::BORDER
                                        },
                                    ))
                                    .corner_radius(10)
                                    .inner_margin(egui::Margin::same(10))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.vertical(|ui| {
                                                ui.label(
                                                    egui::RichText::new(&account.display_name)
                                                        .strong(),
                                                );
                                                ui.label(
                                                    egui::RichText::new(&account.account)
                                                        .small()
                                                        .color(palette::MUTED),
                                                );
                                            });
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    if ui.small_button("删除").clicked() {
                                                        deletion = Some(account.id);
                                                    }
                                                    if selected {
                                                        ui.label(
                                                            egui::RichText::new("使用中")
                                                                .small()
                                                                .color(palette::SUCCESS),
                                                        );
                                                    } else if ui.small_button("切换").clicked() {
                                                        selection =
                                                            Some(AccountMode::Saved(account.id));
                                                    }
                                                },
                                            );
                                        });
                                    });
                                ui.add_space(6.0);
                            }
                            if self.config.accounts.is_empty() {
                                ui.label(
                                    egui::RichText::new("暂无用户，请在右侧添加")
                                        .color(palette::MUTED_DARK),
                                );
                            }
                        });

                    columns[1].label(egui::RichText::new("添加新用户").strong());
                    columns[1].add_space(6.0);
                    columns[1].label(egui::RichText::new("用户名").small().strong());
                    columns[1].add(
                        egui::TextEdit::singleline(&mut self.manager_account)
                            .hint_text("请输入4399账号")
                            .desired_width(f32::INFINITY),
                    );
                    columns[1].label(egui::RichText::new("密码").small().strong());
                    columns[1].add(
                        egui::TextEdit::singleline(&mut self.manager_password)
                            .password(true)
                            .hint_text("请输入账号密码")
                            .desired_width(f32::INFINITY),
                    );
                    columns[1].checkbox(&mut self.manager_save_password, "安全保存到系统密钥环");
                    columns[1].add_space(6.0);
                    if columns[1]
                        .add_sized(
                            [columns[1].available_width(), 42.0],
                            egui::Button::new("＋ 添加并使用")
                                .fill(palette::PRIMARY)
                                .stroke(egui::Stroke::new(1.0_f32, palette::ACCENT)),
                        )
                        .clicked()
                    {
                        add_requested = true;
                    }
                    columns[1].label(
                        egui::RichText::new("密码不写入配置文件；取消勾选后只在本次运行有效")
                            .small()
                            .color(palette::MUTED_DARK),
                    );
                });
            });
        self.account_picker_open = open;
        if let Some(id) = deletion {
            self.delete_managed_account(id, ctx.clone());
        }
        if let Some(mode) = selection {
            self.select_account(mode, ctx.clone());
            self.status = "用户已切换".into();
        } else if add_requested {
            self.add_managed_account(ctx.clone());
        }
    }

    pub(super) fn switch_confirmation(&mut self, ctx: &egui::Context) {
        if !self.confirm_switch {
            return;
        }
        egui::Window::new("确认切换账号")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label("切换账号会立即退出当前游戏，但不会删除游戏资源缓存。");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("取消").clicked() {
                        self.confirm_switch = false;
                    }
                    if ui
                        .add(
                            egui::Button::new("退出并切换")
                                .fill(palette::PRIMARY)
                                .stroke(egui::Stroke::new(1.0_f32, palette::ACCENT)),
                        )
                        .clicked()
                    {
                        self.stop_game("当前游戏已退出，请选择账号".into(), true);
                    }
                });
            });
    }

    pub(super) fn diagnostics_window(&mut self, ctx: &egui::Context) {
        if !self.diagnostics_open {
            return;
        }
        let diagnostics = self.diagnostics();
        egui::Window::new("诊断信息")
            .open(&mut self.diagnostics_open)
            .default_width(640.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(360.0)
                    .show(ui, |ui| ui.monospace(&diagnostics));
                if ui.button("复制").clicked() {
                    ui.ctx().copy_text(diagnostics.clone());
                }
            });
    }
}
