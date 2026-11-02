/// Automatic Cookie-Banner & GDPR Consent Wall Eliminator (`cookie_banner.rs`):
/// 1. Injects high-specificity CSS to hide 60+ annoying cookie popups (OneTrust, Cookiebot,
///    Quantcast, TrustArc, Didomi, Klaro, Sourcepoint, Osano, etc.).
/// 2. Runs a lightweight MutationObserver & auto-clicker that clicks "Reject All" / "Necessary Only"
///    or "Accept" and forces `overflow: auto !important` on `<html>` and `<body>` so pages never freeze!
pub struct CookieBannerEliminator;

impl CookieBannerEliminator {
    pub const COSMETIC_COOKIE_CSS: &'static str = r#"
        #onetrust-consent-sdk, #onetrust-banner-sdk, .onetrust-pc-dark-filter,
        #CybotCookiebotDialog, #CybotCookiebotDialogBodyUnderlay,
        #didomi-host, #didomi-popup, .didomi-popup-backdrop,
        .qc-cmp2-container, .qc-cmp-cleanslate,
        #truste-consent-track, .truste_box_overlay, .truste_overlay,
        .cc-window, .cc-banner, .cc-floating,
        #cookie-law-info-bar, #cookie-notice, #cookiebanner, #cookie-banner,
        .js-cookie-consent, .cookie-consent-banner, .cookie-modal,
        [id*="sp_message_container"], .osano-cm-window, .klaro .cookie-modal {
            display: none !important;
            visibility: hidden !important;
            opacity: 0 !important;
            pointer-events: none !important;
            z-index: -9999 !important;
        }
    "#;

    pub const AUTO_DISMISS_COOKIE_JS: &'static str = r#"
        (function() {
            const unlockScroll = function() {
                if (!document.body || !document.documentElement) return;
                const bodyStyle = window.getComputedStyle(document.body);
                const htmlStyle = window.getComputedStyle(document.documentElement);
                if (bodyStyle.overflow === 'hidden' || htmlStyle.overflow === 'hidden' ||
                    document.body.classList.contains('sp-message-open') ||
                    document.documentElement.classList.contains('sp-message-open')) {
                    document.documentElement.style.setProperty('overflow', 'auto', 'important');
                    document.body.style.setProperty('overflow', 'auto', 'important');
                    document.body.style.setProperty('position', 'static', 'important');
                }
            };

            const dismissSelectors = [
                '#onetrust-reject-all-handler',
                '#onetrust-accept-btn-handler',
                '#CybotCookiebotDialogBodyButtonDecline',
                '#CybotCookiebotDialogBodyLevelButtonLevelOptinAllowallSelection',
                '#didomi-notice-disagree-button',
                '#didomi-notice-agree-button',
                '.qc-cmp2-summary-buttons button[mode="secondary"]',
                '#cookie_action_close_header_reject',
                '#cookie_action_close_header',
                'button[data-testid="cookie-policy-dialog-accept-button"]',
                'button[aria-label*="Reject" i]',
                'button[aria-label*="Accept cookie" i]'
            ];

            const sweepBanners = function() {
                for (let i = 0; i < dismissSelectors.length; i++) {
                    try {
                        const btn = document.querySelector(dismissSelectors[i]);
                        if (btn && typeof btn.click === 'function') {
                            btn.click();
                        }
                    } catch (_) {}
                }
                unlockScroll();
            };

            if (document.readyState === 'loading') {
                document.addEventListener('DOMContentLoaded', sweepBanners, { once: true });
            } else {
                sweepBanners();
            }
            setTimeout(sweepBanners, 650);
            setTimeout(sweepBanners, 1800);
        })();
    "#;
}
