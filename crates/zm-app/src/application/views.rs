use super::*;

impl ZmApp {
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
        let viewport = ctx.content_rect();
        let window_width = (viewport.width() - 32.0).clamp(320.0, 720.0);
        let compact = viewport.width() < palette::control::RESPONSIVE_BREAKPOINT;
        let window_height = if compact {
            (viewport.height() - 64.0).clamp(420.0, 540.0)
        } else {
            360.0_f32.min(viewport.height() - 64.0).max(300.0)
        };
        let account_list_height = if compact { 190.0 } else { 224.0 };
        let mut open = self.account_picker_open;
        let mut selection = None;
        let mut deletion = None;
        let mut add_requested = false;
        egui::Window::new("账号管理")
            .collapsible(false)
            .resizable(false)
            .fixed_size(egui::vec2(window_width, window_height))
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .open(&mut open)
            .show(ctx, |ui| {
                let show_saved_accounts =
                    |ui: &mut egui::Ui,
                     app: &ZmApp,
                     selection: &mut Option<AccountMode>,
                     deletion: &mut Option<Uuid>| {
                        ui.label(egui::RichText::new("已保存用户").strong());
                        ui.add_space(6.0);
                        egui::ScrollArea::vertical()
                            .id_salt("saved-accounts")
                            .min_scrolled_height(account_list_height)
                            .max_height(account_list_height)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                for account in &app.config.accounts {
                                    let selected =
                                        app.account_mode == AccountMode::Saved(account.id);
                                    egui::Frame::new()
                                        .fill(if selected {
                                            palette::SURFACE_HOVERED
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
                                                let information_width =
                                                    (ui.available_width() - 108.0).max(72.0);
                                                ui.allocate_ui_with_layout(
                                                    egui::vec2(information_width, 38.0),
                                                    egui::Layout::top_down(egui::Align::Min),
                                                    |ui| {
                                                        ui.add(
                                                            egui::Label::new(
                                                                egui::RichText::new(
                                                                    &account.display_name,
                                                                )
                                                                .strong(),
                                                            )
                                                            .truncate(),
                                                        )
                                                        .on_hover_text(&account.display_name);
                                                        ui.add(
                                                            egui::Label::new(
                                                                egui::RichText::new(
                                                                    &account.account,
                                                                )
                                                                .small()
                                                                .color(palette::MUTED),
                                                            )
                                                            .truncate(),
                                                        )
                                                        .on_hover_text(&account.account);
                                                    },
                                                );
                                                ui.with_layout(
                                                    egui::Layout::right_to_left(
                                                        egui::Align::Center,
                                                    ),
                                                    |ui| {
                                                        if ui.small_button("删除").clicked() {
                                                            *deletion = Some(account.id);
                                                        }
                                                        if selected {
                                                            ui.label(
                                                                egui::RichText::new("使用中")
                                                                    .small()
                                                                    .color(palette::SUCCESS),
                                                            );
                                                        } else if ui.small_button("切换").clicked()
                                                        {
                                                            *selection = Some(AccountMode::Saved(
                                                                account.id,
                                                            ));
                                                        }
                                                    },
                                                );
                                            });
                                        });
                                    ui.add_space(6.0);
                                }
                                if app.config.accounts.is_empty() {
                                    ui.label(
                                        egui::RichText::new("暂无用户，请添加新用户")
                                            .color(palette::MUTED_DARK),
                                    );
                                }
                            });
                    };

                let show_add_account =
                    |ui: &mut egui::Ui, app: &mut ZmApp, add_requested: &mut bool| {
                        ui.label(egui::RichText::new("添加新用户").strong());
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new("用户名").small().strong());
                        ui.add(
                            egui::TextEdit::singleline(&mut app.manager_account)
                                .hint_text("请输入4399账号")
                                .desired_width(f32::INFINITY),
                        );
                        ui.label(egui::RichText::new("密码").small().strong());
                        ui.add(
                            egui::TextEdit::singleline(&mut app.manager_password)
                                .password(true)
                                .hint_text("请输入账号密码")
                                .desired_width(f32::INFINITY),
                        );
                        ui.checkbox(&mut app.manager_save_password, "安全保存到系统密钥环");
                        ui.add_space(6.0);
                        if ui
                            .add_sized(
                                [ui.available_width(), 42.0],
                                egui::Button::new("＋ 添加并使用")
                                    .fill(palette::PRIMARY)
                                    .stroke(egui::Stroke::new(1.0_f32, palette::ACCENT)),
                            )
                            .clicked()
                        {
                            *add_requested = true;
                        }
                        ui.label(
                            egui::RichText::new("密码不写入配置文件；取消勾选后只在本次运行有效")
                                .small()
                                .color(palette::MUTED_DARK),
                        );
                    };

                ui.label(egui::RichText::new("集中管理用于登录4399的账号").color(palette::MUTED));
                ui.add_space(10.0);
                if compact {
                    egui::ScrollArea::vertical()
                        .id_salt("account-picker-body")
                        .max_height(window_height - 76.0)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            show_saved_accounts(ui, self, &mut selection, &mut deletion);
                            ui.add_space(10.0);
                            ui.separator();
                            ui.add_space(10.0);
                            show_add_account(ui, self, &mut add_requested);
                        });
                } else {
                    ui.columns(2, |columns| {
                        show_saved_accounts(&mut columns[0], self, &mut selection, &mut deletion);
                        show_add_account(&mut columns[1], self, &mut add_requested);
                    });
                }
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
