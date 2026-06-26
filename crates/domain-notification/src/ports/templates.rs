use handlebars::Handlebars;

/// Registry of embedded notification templates (handlebars, strict mode).
pub struct Templates {
    hb: Handlebars<'static>,
}

impl Templates {
    pub fn new() -> anyhow::Result<Templates> {
        let mut hb = Handlebars::new();
        hb.set_strict_mode(true); // missing variable -> render error -> DLQ
        hb.register_template_string("welcome", include_str!("../templates/welcome.txt.hbs"))?;
        Ok(Templates { hb })
    }

    pub fn render(&self, name: &str, vars: &serde_json::Value) -> anyhow::Result<String> {
        if !self.hb.has_template(name) {
            anyhow::bail!("unknown template: {name}");
        }
        Ok(self.hb.render(name, vars)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_welcome_with_vars() {
        let t = Templates::new().unwrap();
        let body = t
            .render(
                "welcome",
                &serde_json::json!({ "email": "a@b.c", "account_id": 7 }),
            )
            .unwrap();
        assert!(body.contains("a@b.c"));
        assert!(body.contains("#7"));
    }

    #[test]
    fn unknown_template_errors() {
        let t = Templates::new().unwrap();
        assert!(t.render("nope", &serde_json::json!({})).is_err());
    }

    #[test]
    fn missing_variable_errors_in_strict_mode() {
        let t = Templates::new().unwrap();
        assert!(t
            .render("welcome", &serde_json::json!({ "email": "a@b.c" }))
            .is_err());
    }
}
