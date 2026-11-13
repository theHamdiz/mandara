//! Small path adaptations for packages installed outside their original client.

use super::manifest::Manifest;

pub(super) fn adapt(id: &str, manifest: &mut Manifest) {
    if id == "qutebrowser" {
        for template in manifest.templates.values_mut() {
            if template
                .post_hook
                .contains("community-templates/qutebrowser/reload.sh")
            {
                template.post_hook = "sh '{{ config_dir }}/reload.sh'".into();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reload_scripts_follow_the_installed_package_without_rewriting_other_hooks() {
        let mut manifest = Manifest::parse("[templates.browser]\ninput_path = 'colors.py'\noutput_path = '~/colors.py'\npost_hook = 'sh /state/community-templates/qutebrowser/reload.sh'\n").unwrap();
        adapt("qutebrowser", &mut manifest);
        assert_eq!(
            manifest.templates["browser"].post_hook,
            "sh '{{ config_dir }}/reload.sh'"
        );
        adapt("other", &mut manifest);
        assert_eq!(
            manifest.templates["browser"].post_hook,
            "sh '{{ config_dir }}/reload.sh'"
        );
    }
}
