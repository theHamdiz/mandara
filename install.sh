#!/bin/sh
# Downloads a verified release; Mandara's Rust installer handles the rest.
set -eu
umask 077

release_keys='2fde15bd0518f2352436174a3bcc1e41d62e3ac99647a9dfa773d6462970b43d'
release_base=${MANDARA_BOOTSTRAP_BASE_URL:-https://github.com/theHamdiz/mandara/releases/latest/download}
work=
language=${LC_ALL:-${LC_MESSAGES:-${LANG:-en}}}
language=${language%%[_.@-]*}

# Generated messages: cargo xtask bootstrap
message() {
    case "$language:$1" in
        ar:welcome) printf '%s\n' 'سطح مكتب Wayland الخاص بك. لنجهزه معًا.' ;;
        ar:signature) printf '%s\n' 'جارٍ التحقق من توقيع الإصدار…' ;;
        ar:download) printf '%s\n' 'جارٍ تنزيل Mandara…' ;;
        ar:verify) printf '%s\n' 'جارٍ التحقق من التنزيل…' ;;
        ar:unpack) printf '%s\n' 'جارٍ فك ملفات المثبّت…' ;;
        ar:start) printf '%s\n' 'كل شيء جاهز. جارٍ فتح مثبّت Mandara…' ;;
        ar:missing_tool) printf '%s\n' 'ثبّت هذه الأداة ثم أعد المحاولة:' ;;
        ar:unsupported) printf '%s\n' 'تتطلب الإصدارات الجاهزة Linux ومعالج x86_64 أو aarch64.' ;;
        ar:https) printf '%s\n' 'يجب أن تستخدم مرآة الإصدارات HTTPS.' ;;
        ar:temporary) printf '%s\n' 'تعذر إنشاء مجلد مؤقت للتنزيل.' ;;
        ar:failed_download) printf '%s\n' 'فشل التنزيل. تحقق من اتصالك وأعد المحاولة.' ;;
        ar:bad_signature) printf '%s\n' 'تعذر التحقق من توقيع الإصدار. لم يُثبّت أي شيء.' ;;
        ar:openssl) printf '%s\n' 'يتطلب هذا الأمر OpenSSL 3 أو أحدث.' ;;
        ar:missing_checksum) printf '%s\n' 'لا يحتوي الإصدار الموقّع على بصمة تحقق واحدة لهذا الأرشيف.' ;;
        ar:bad_checksum) printf '%s\n' 'لا يطابق التنزيل الإصدار الموقّع. لم يُثبّت أي شيء؛ أعد المحاولة.' ;;
        ar:failed_unpack) printf '%s\n' 'تعذر فك أرشيف الإصدار الذي تم التحقق منه.' ;;
        ar:missing_installer) printf '%s\n' 'لا يحتوي الإصدار على مثبّت تنفيذي واحد بالضبط.' ;;
        ar:manual) printf '%s\n' 'التنزيل اليدوي:' ;;
        ar:nixos) printf '%s\n' 'على NixOS، ثبّت Mandara من خلال إعدادات flake.' ;;
        fa:welcome) printf '%s\n' 'میزکار Wayland شما. بیایید آماده‌اش کنیم.' ;;
        fa:signature) printf '%s\n' 'در حال بررسی امضای انتشار…' ;;
        fa:download) printf '%s\n' 'در حال دریافت Mandara…' ;;
        fa:verify) printf '%s\n' 'در حال بررسی فایل دریافتی…' ;;
        fa:unpack) printf '%s\n' 'در حال استخراج فایل‌های نصب…' ;;
        fa:start) printf '%s\n' 'آماده است. در حال باز کردن نصب‌کنندهٔ Mandara…' ;;
        fa:missing_tool) printf '%s\n' 'این ابزار را نصب و دوباره تلاش کنید:' ;;
        fa:unsupported) printf '%s\n' 'نسخه‌های آماده به Linux با پردازندهٔ x86_64 یا aarch64 نیاز دارند.' ;;
        fa:https) printf '%s\n' 'آینهٔ انتشار باید از HTTPS استفاده کند.' ;;
        fa:temporary) printf '%s\n' 'ساخت پوشهٔ موقت دریافت ممکن نشد.' ;;
        fa:failed_download) printf '%s\n' 'دریافت ناموفق بود. اتصال خود را بررسی و دوباره تلاش کنید.' ;;
        fa:bad_signature) printf '%s\n' 'امضای انتشار تأیید نشد. چیزی نصب نشد.' ;;
        fa:openssl) printf '%s\n' 'این فرمان به OpenSSL 3 یا جدیدتر نیاز دارد.' ;;
        fa:missing_checksum) printf '%s\n' 'انتشار امضاشده برای این بایگانی چک‌سام یکتایی ندارد.' ;;
        fa:bad_checksum) printf '%s\n' 'فایل دریافتی با انتشار امضاشده مطابقت ندارد. چیزی نصب نشد؛ دوباره تلاش کنید.' ;;
        fa:failed_unpack) printf '%s\n' 'استخراج بایگانی تأییدشده ممکن نشد.' ;;
        fa:missing_installer) printf '%s\n' 'تعداد نصب‌کننده‌های قابل اجرای این انتشار دقیقاً یک نیست.' ;;
        fa:manual) printf '%s\n' 'دریافت دستی:' ;;
        fa:nixos) printf '%s\n' 'در NixOS، Mandara را از طریق flake خود نصب کنید.' ;;
        ur:welcome) printf '%s\n' 'آپ کا Wayland ڈیسک ٹاپ۔ آئیے اسے تیار کریں۔' ;;
        ur:signature) printf '%s\n' 'ریلیز کے دستخط کی تصدیق ہو رہی ہے…' ;;
        ur:download) printf '%s\n' 'Mandara ڈاؤن لوڈ ہو رہا ہے…' ;;
        ur:verify) printf '%s\n' 'ڈاؤن لوڈ کی تصدیق ہو رہی ہے…' ;;
        ur:unpack) printf '%s\n' 'انسٹالر کی فائلیں نکالی جا رہی ہیں…' ;;
        ur:start) printf '%s\n' 'تیار ہے۔ Mandara انسٹالر کھولا جا رہا ہے…' ;;
        ur:missing_tool) printf '%s\n' 'یہ ٹول نصب کریں اور دوبارہ کوشش کریں:' ;;
        ur:unsupported) printf '%s\n' 'تیار ریلیز کے لیے Linux اور x86_64 یا aarch64 پروسیسر درکار ہے۔' ;;
        ur:https) printf '%s\n' 'ریلیز مرر کو HTTPS استعمال کرنا چاہیے۔' ;;
        ur:temporary) printf '%s\n' 'ڈاؤن لوڈ کے لیے عارضی فولڈر نہیں بنایا جا سکا۔' ;;
        ur:failed_download) printf '%s\n' 'ڈاؤن لوڈ ناکام ہوا۔ اپنا کنکشن چیک کریں اور دوبارہ کوشش کریں۔' ;;
        ur:bad_signature) printf '%s\n' 'ریلیز کے دستخط کی تصدیق نہیں ہو سکی۔ کچھ بھی نصب نہیں ہوا۔' ;;
        ur:openssl) printf '%s\n' 'اس کمانڈ کے لیے OpenSSL 3 یا جدید تر درکار ہے۔' ;;
        ur:missing_checksum) printf '%s\n' 'دستخط شدہ ریلیز میں اس آرکائیو کے لیے منفرد چیک سم موجود نہیں۔' ;;
        ur:bad_checksum) printf '%s\n' 'ڈاؤن لوڈ دستخط شدہ ریلیز سے مطابقت نہیں رکھتا۔ کچھ بھی نصب نہیں ہوا؛ دوبارہ کوشش کریں۔' ;;
        ur:failed_unpack) printf '%s\n' 'تصدیق شدہ ریلیز کی فائلیں نہیں نکالی جا سکیں۔' ;;
        ur:missing_installer) printf '%s\n' 'ریلیز میں قابلِ اجرا انسٹالرز کی تعداد ایک نہیں ہے۔' ;;
        ur:manual) printf '%s\n' 'دستی ڈاؤن لوڈ:' ;;
        ur:nixos) printf '%s\n' 'NixOS پر Mandara اپنی flake کے ذریعے نصب کریں۔' ;;
        es:welcome) printf '%s\n' 'Tu escritorio Wayland. Vamos a prepararlo.' ;;
        es:signature) printf '%s\n' 'Comprobando la firma de la versión…' ;;
        es:download) printf '%s\n' 'Descargando Mandara…' ;;
        es:verify) printf '%s\n' 'Verificando la descarga…' ;;
        es:unpack) printf '%s\n' 'Descomprimiendo el instalador…' ;;
        es:start) printf '%s\n' 'Todo listo. Abriendo el instalador de Mandara…' ;;
        es:missing_tool) printf '%s\n' 'Instala esta herramienta y vuelve a intentarlo:' ;;
        es:unsupported) printf '%s\n' 'Las versiones precompiladas requieren Linux con una CPU x86_64 o aarch64.' ;;
        es:https) printf '%s\n' 'El servidor de versiones alternativo debe usar HTTPS.' ;;
        es:temporary) printf '%s\n' 'No se pudo crear una carpeta temporal para la descarga.' ;;
        es:failed_download) printf '%s\n' 'La descarga falló. Comprueba tu conexión y vuelve a intentarlo.' ;;
        es:bad_signature) printf '%s\n' 'No se pudo verificar la firma de la versión. No se instaló nada.' ;;
        es:openssl) printf '%s\n' 'Este comando necesita OpenSSL 3 o posterior.' ;;
        es:missing_checksum) printf '%s\n' 'La versión firmada no tiene una suma de verificación única para este archivo.' ;;
        es:bad_checksum) printf '%s\n' 'La descarga no coincide con la versión firmada. No se instaló nada; vuelve a intentarlo.' ;;
        es:failed_unpack) printf '%s\n' 'No se pudo descomprimir la versión verificada.' ;;
        es:missing_installer) printf '%s\n' 'La versión no contiene exactamente un instalador ejecutable.' ;;
        es:manual) printf '%s\n' 'Descargas manuales:' ;;
        es:nixos) printf '%s\n' 'En NixOS, instala Mandara mediante tu flake.' ;;
        fr:welcome) printf '%s\n' 'Votre bureau Wayland. Préparons-le ensemble.' ;;
        fr:signature) printf '%s\n' 'Vérification de la signature de la version…' ;;
        fr:download) printf '%s\n' 'Téléchargement de Mandara…' ;;
        fr:verify) printf '%s\n' 'Vérification du téléchargement…' ;;
        fr:unpack) printf '%s\n' 'Décompression du programme d’installation…' ;;
        fr:start) printf '%s\n' 'Tout est prêt. Ouverture du programme d’installation de Mandara…' ;;
        fr:missing_tool) printf '%s\n' 'Installez cet outil, puis réessayez :' ;;
        fr:unsupported) printf '%s\n' 'Les versions précompilées nécessitent Linux et un processeur x86_64 ou aarch64.' ;;
        fr:https) printf '%s\n' 'Le miroir des versions doit utiliser HTTPS.' ;;
        fr:temporary) printf '%s\n' 'Impossible de créer un dossier temporaire pour le téléchargement.' ;;
        fr:failed_download) printf '%s\n' 'Le téléchargement a échoué. Vérifiez votre connexion et réessayez.' ;;
        fr:bad_signature) printf '%s\n' 'La signature de la version n’a pas pu être vérifiée. Rien n’a été installé.' ;;
        fr:openssl) printf '%s\n' 'Cette commande nécessite OpenSSL 3 ou une version plus récente.' ;;
        fr:missing_checksum) printf '%s\n' 'La version signée ne contient pas de somme de contrôle unique pour cette archive.' ;;
        fr:bad_checksum) printf '%s\n' 'Le téléchargement ne correspond pas à la version signée. Rien n’a été installé ; réessayez.' ;;
        fr:failed_unpack) printf '%s\n' 'Impossible de décompresser la version vérifiée.' ;;
        fr:missing_installer) printf '%s\n' 'La version ne contient pas exactement un programme d’installation exécutable.' ;;
        fr:manual) printf '%s\n' 'Téléchargements manuels :' ;;
        fr:nixos) printf '%s\n' 'Sur NixOS, installez Mandara avec votre flake.' ;;
        de:welcome) printf '%s\n' 'Dein Wayland-Desktop. Machen wir ihn startklar.' ;;
        de:signature) printf '%s\n' 'Signatur der Veröffentlichung wird geprüft…' ;;
        de:download) printf '%s\n' 'Mandara wird heruntergeladen…' ;;
        de:verify) printf '%s\n' 'Download wird geprüft…' ;;
        de:unpack) printf '%s\n' 'Installationsprogramm wird entpackt…' ;;
        de:start) printf '%s\n' 'Alles bereit. Das Mandara-Installationsprogramm wird geöffnet…' ;;
        de:missing_tool) printf '%s\n' 'Installiere dieses Werkzeug und versuche es erneut:' ;;
        de:unsupported) printf '%s\n' 'Vorgefertigte Versionen benötigen Linux mit einer x86_64- oder aarch64-CPU.' ;;
        de:https) printf '%s\n' 'Der Download-Spiegel muss HTTPS verwenden.' ;;
        de:temporary) printf '%s\n' 'Ein temporärer Download-Ordner konnte nicht erstellt werden.' ;;
        de:failed_download) printf '%s\n' 'Download fehlgeschlagen. Prüfe deine Verbindung und versuche es erneut.' ;;
        de:bad_signature) printf '%s\n' 'Die Signatur der Veröffentlichung konnte nicht geprüft werden. Es wurde nichts installiert.' ;;
        de:openssl) printf '%s\n' 'Dieser Befehl benötigt OpenSSL 3 oder neuer.' ;;
        de:missing_checksum) printf '%s\n' 'Die signierte Veröffentlichung enthält keine eindeutige Prüfsumme für dieses Archiv.' ;;
        de:bad_checksum) printf '%s\n' 'Der Download stimmt nicht mit der signierten Veröffentlichung überein. Es wurde nichts installiert; versuche es erneut.' ;;
        de:failed_unpack) printf '%s\n' 'Die geprüfte Veröffentlichung konnte nicht entpackt werden.' ;;
        de:missing_installer) printf '%s\n' 'Die Veröffentlichung enthält nicht genau ein ausführbares Installationsprogramm.' ;;
        de:manual) printf '%s\n' 'Manuelle Downloads:' ;;
        de:nixos) printf '%s\n' 'Installiere Mandara unter NixOS über deinen Flake.' ;;
        tr:welcome) printf '%s\n' 'Wayland masaüstünüz. Birlikte hazırlayalım.' ;;
        tr:signature) printf '%s\n' 'Sürüm imzası denetleniyor…' ;;
        tr:download) printf '%s\n' 'Mandara indiriliyor…' ;;
        tr:verify) printf '%s\n' 'İndirme doğrulanıyor…' ;;
        tr:unpack) printf '%s\n' 'Kurulum dosyaları arşivden çıkarılıyor…' ;;
        tr:start) printf '%s\n' 'Hazır. Mandara kurulum programı açılıyor…' ;;
        tr:missing_tool) printf '%s\n' 'Bu aracı yükleyip yeniden deneyin:' ;;
        tr:unsupported) printf '%s\n' 'Hazır sürümler için Linux ve x86_64 veya aarch64 işlemci gerekir.' ;;
        tr:https) printf '%s\n' 'Sürüm aynası HTTPS kullanmalıdır.' ;;
        tr:temporary) printf '%s\n' 'Geçici indirme klasörü oluşturulamadı.' ;;
        tr:failed_download) printf '%s\n' 'İndirme başarısız oldu. Bağlantınızı denetleyip yeniden deneyin.' ;;
        tr:bad_signature) printf '%s\n' 'Sürüm imzası doğrulanamadı. Hiçbir şey yüklenmedi.' ;;
        tr:openssl) printf '%s\n' 'Bu komut için OpenSSL 3 veya daha yenisi gerekir.' ;;
        tr:missing_checksum) printf '%s\n' 'İmzalı sürümde bu arşiv için tek bir sağlama toplamı bulunmuyor.' ;;
        tr:bad_checksum) printf '%s\n' 'İndirme, imzalı sürümle eşleşmiyor. Hiçbir şey yüklenmedi; yeniden deneyin.' ;;
        tr:failed_unpack) printf '%s\n' 'Doğrulanmış sürüm arşivden çıkarılamadı.' ;;
        tr:missing_installer) printf '%s\n' 'Sürümdeki çalıştırılabilir kurulum programı sayısı tam olarak bir değil.' ;;
        tr:manual) printf '%s\n' 'Elle indirme:' ;;
        tr:nixos) printf '%s\n' 'NixOS üzerinde Mandara’yı flake yapılandırmanızla yükleyin.' ;;
        ru:welcome) printf '%s\n' 'Ваш рабочий стол Wayland. Подготовим его к работе.' ;;
        ru:signature) printf '%s\n' 'Проверка подписи выпуска…' ;;
        ru:download) printf '%s\n' 'Загрузка Mandara…' ;;
        ru:verify) printf '%s\n' 'Проверка загрузки…' ;;
        ru:unpack) printf '%s\n' 'Распаковка установщика…' ;;
        ru:start) printf '%s\n' 'Всё готово. Запуск установщика Mandara…' ;;
        ru:missing_tool) printf '%s\n' 'Установите этот инструмент и повторите попытку:' ;;
        ru:unsupported) printf '%s\n' 'Готовые сборки требуют Linux и процессор x86_64 или aarch64.' ;;
        ru:https) printf '%s\n' 'Зеркало выпусков должно использовать HTTPS.' ;;
        ru:temporary) printf '%s\n' 'Не удалось создать временную папку для загрузки.' ;;
        ru:failed_download) printf '%s\n' 'Загрузка не удалась. Проверьте соединение и повторите попытку.' ;;
        ru:bad_signature) printf '%s\n' 'Не удалось проверить подпись выпуска. Ничего не установлено.' ;;
        ru:openssl) printf '%s\n' 'Для этой команды нужен OpenSSL 3 или новее.' ;;
        ru:missing_checksum) printf '%s\n' 'В подписанном выпуске нет единственной контрольной суммы для этого архива.' ;;
        ru:bad_checksum) printf '%s\n' 'Загрузка не соответствует подписанному выпуску. Ничего не установлено; повторите попытку.' ;;
        ru:failed_unpack) printf '%s\n' 'Не удалось распаковать проверенный выпуск.' ;;
        ru:missing_installer) printf '%s\n' 'Количество исполняемых установщиков в выпуске не равно одному.' ;;
        ru:manual) printf '%s\n' 'Загрузка вручную:' ;;
        ru:nixos) printf '%s\n' 'В NixOS установите Mandara через свой flake.' ;;
        hi:welcome) printf '%s\n' 'आपका Wayland डेस्कटॉप। आइए इसे तैयार करें।' ;;
        hi:signature) printf '%s\n' 'रिलीज़ के हस्ताक्षर की जाँच हो रही है…' ;;
        hi:download) printf '%s\n' 'Mandara डाउनलोड हो रहा है…' ;;
        hi:verify) printf '%s\n' 'डाउनलोड की जाँच हो रही है…' ;;
        hi:unpack) printf '%s\n' 'इंस्टॉलर की फ़ाइलें निकाली जा रही हैं…' ;;
        hi:start) printf '%s\n' 'तैयार है। Mandara इंस्टॉलर खुल रहा है…' ;;
        hi:missing_tool) printf '%s\n' 'यह टूल इंस्टॉल करें और फिर कोशिश करें:' ;;
        hi:unsupported) printf '%s\n' 'तैयार रिलीज़ के लिए Linux और x86_64 या aarch64 CPU चाहिए।' ;;
        hi:https) printf '%s\n' 'रिलीज़ मिरर को HTTPS इस्तेमाल करना चाहिए।' ;;
        hi:temporary) printf '%s\n' 'अस्थायी डाउनलोड फ़ोल्डर नहीं बनाया जा सका।' ;;
        hi:failed_download) printf '%s\n' 'डाउनलोड विफल रहा। अपना कनेक्शन जाँचें और फिर कोशिश करें।' ;;
        hi:bad_signature) printf '%s\n' 'रिलीज़ के हस्ताक्षर की पुष्टि नहीं हो सकी। कुछ भी इंस्टॉल नहीं हुआ।' ;;
        hi:openssl) printf '%s\n' 'इस कमांड के लिए OpenSSL 3 या नया संस्करण चाहिए।' ;;
        hi:missing_checksum) printf '%s\n' 'हस्ताक्षरित रिलीज़ में इस आर्काइव के लिए एकमात्र चेकसम नहीं है।' ;;
        hi:bad_checksum) printf '%s\n' 'डाउनलोड हस्ताक्षरित रिलीज़ से मेल नहीं खाता। कुछ भी इंस्टॉल नहीं हुआ; फिर कोशिश करें।' ;;
        hi:failed_unpack) printf '%s\n' 'सत्यापित रिलीज़ की फ़ाइलें नहीं निकाली जा सकीं।' ;;
        hi:missing_installer) printf '%s\n' 'रिलीज़ में चलने योग्य इंस्टॉलर की संख्या ठीक एक नहीं है।' ;;
        hi:manual) printf '%s\n' 'मैन्युअल डाउनलोड:' ;;
        hi:nixos) printf '%s\n' 'NixOS पर अपनी flake से Mandara इंस्टॉल करें।' ;;
        bn:welcome) printf '%s\n' 'আপনার Wayland ডেস্কটপ। চলুন প্রস্তুত করি।' ;;
        bn:signature) printf '%s\n' 'রিলিজের স্বাক্ষর যাচাই করা হচ্ছে…' ;;
        bn:download) printf '%s\n' 'Mandara ডাউনলোড করা হচ্ছে…' ;;
        bn:verify) printf '%s\n' 'ডাউনলোড যাচাই করা হচ্ছে…' ;;
        bn:unpack) printf '%s\n' 'ইনস্টলারের ফাইল আনপ্যাক করা হচ্ছে…' ;;
        bn:start) printf '%s\n' 'প্রস্তুত। Mandara ইনস্টলার খোলা হচ্ছে…' ;;
        bn:missing_tool) printf '%s\n' 'এই টুলটি ইনস্টল করে আবার চেষ্টা করুন:' ;;
        bn:unsupported) printf '%s\n' 'আগে থেকে তৈরি রিলিজের জন্য Linux এবং x86_64 বা aarch64 CPU প্রয়োজন।' ;;
        bn:https) printf '%s\n' 'রিলিজ মিররে HTTPS ব্যবহার করতে হবে।' ;;
        bn:temporary) printf '%s\n' 'ডাউনলোডের জন্য অস্থায়ী ফোল্ডার তৈরি করা যায়নি।' ;;
        bn:failed_download) printf '%s\n' 'ডাউনলোড ব্যর্থ হয়েছে। সংযোগ পরীক্ষা করে আবার চেষ্টা করুন।' ;;
        bn:bad_signature) printf '%s\n' 'রিলিজের স্বাক্ষর যাচাই করা যায়নি। কিছুই ইনস্টল করা হয়নি।' ;;
        bn:openssl) printf '%s\n' 'এই কমান্ডের জন্য OpenSSL 3 বা পরবর্তী সংস্করণ প্রয়োজন।' ;;
        bn:missing_checksum) printf '%s\n' 'স্বাক্ষরিত রিলিজে এই আর্কাইভের জন্য একটিমাত্র চেকসাম নেই।' ;;
        bn:bad_checksum) printf '%s\n' 'ডাউনলোডটি স্বাক্ষরিত রিলিজের সঙ্গে মেলেনি। কিছুই ইনস্টল করা হয়নি; আবার চেষ্টা করুন।' ;;
        bn:failed_unpack) printf '%s\n' 'যাচাই করা রিলিজের ফাইল আনপ্যাক করা যায়নি।' ;;
        bn:missing_installer) printf '%s\n' 'রিলিজে চালানো যায় এমন ইনস্টলারের সংখ্যা ঠিক এক নয়।' ;;
        bn:manual) printf '%s\n' 'নিজে ডাউনলোড করুন:' ;;
        bn:nixos) printf '%s\n' 'NixOS-এ আপনার flake দিয়ে Mandara ইনস্টল করুন।' ;;
        id:welcome) printf '%s\n' 'Desktop Wayland Anda. Mari kita siapkan.' ;;
        id:signature) printf '%s\n' 'Memeriksa tanda tangan rilis…' ;;
        id:download) printf '%s\n' 'Mengunduh Mandara…' ;;
        id:verify) printf '%s\n' 'Memverifikasi unduhan…' ;;
        id:unpack) printf '%s\n' 'Mengekstrak pemasang…' ;;
        id:start) printf '%s\n' 'Siap. Membuka pemasang Mandara…' ;;
        id:missing_tool) printf '%s\n' 'Pasang alat ini lalu coba lagi:' ;;
        id:unsupported) printf '%s\n' 'Rilis siap pakai memerlukan Linux dengan CPU x86_64 atau aarch64.' ;;
        id:https) printf '%s\n' 'Cermin rilis harus menggunakan HTTPS.' ;;
        id:temporary) printf '%s\n' 'Tidak dapat membuat folder unduhan sementara.' ;;
        id:failed_download) printf '%s\n' 'Unduhan gagal. Periksa koneksi Anda dan coba lagi.' ;;
        id:bad_signature) printf '%s\n' 'Tanda tangan rilis tidak dapat diverifikasi. Tidak ada yang dipasang.' ;;
        id:openssl) printf '%s\n' 'Perintah ini memerlukan OpenSSL 3 atau yang lebih baru.' ;;
        id:missing_checksum) printf '%s\n' 'Rilis bertanda tangan tidak memiliki satu checksum unik untuk arsip ini.' ;;
        id:bad_checksum) printf '%s\n' 'Unduhan tidak cocok dengan rilis bertanda tangan. Tidak ada yang dipasang; silakan coba lagi.' ;;
        id:failed_unpack) printf '%s\n' 'Tidak dapat mengekstrak rilis yang telah diverifikasi.' ;;
        id:missing_installer) printf '%s\n' 'Rilis tidak berisi tepat satu pemasang yang dapat dijalankan.' ;;
        id:manual) printf '%s\n' 'Unduhan manual:' ;;
        id:nixos) printf '%s\n' 'Di NixOS, pasang Mandara melalui flake Anda.' ;;
        zh:welcome) printf '%s\n' '你的 Wayland 桌面。让我们准备就绪。' ;;
        zh:signature) printf '%s\n' '正在检查发布版本签名…' ;;
        zh:download) printf '%s\n' '正在下载 Mandara…' ;;
        zh:verify) printf '%s\n' '正在验证下载…' ;;
        zh:unpack) printf '%s\n' '正在解压安装程序…' ;;
        zh:start) printf '%s\n' '准备就绪。正在打开 Mandara 安装程序…' ;;
        zh:missing_tool) printf '%s\n' '请安装此工具后重试：' ;;
        zh:unsupported) printf '%s\n' '预编译版本需要 Linux 系统和 x86_64 或 aarch64 处理器。' ;;
        zh:https) printf '%s\n' '发布镜像必须使用 HTTPS。' ;;
        zh:temporary) printf '%s\n' '无法创建临时下载文件夹。' ;;
        zh:failed_download) printf '%s\n' '下载失败。请检查网络连接后重试。' ;;
        zh:bad_signature) printf '%s\n' '无法验证发布版本签名。未安装任何内容。' ;;
        zh:openssl) printf '%s\n' '此命令需要 OpenSSL 3 或更新版本。' ;;
        zh:missing_checksum) printf '%s\n' '已签名的发布版本中没有此归档文件的唯一校验和。' ;;
        zh:bad_checksum) printf '%s\n' '下载内容与已签名的发布版本不匹配。未安装任何内容；请重试。' ;;
        zh:failed_unpack) printf '%s\n' '无法解压已验证的发布版本。' ;;
        zh:missing_installer) printf '%s\n' '发布版本中的可执行安装程序数量不是一个。' ;;
        zh:manual) printf '%s\n' '手动下载：' ;;
        zh:nixos) printf '%s\n' '在 NixOS 上，请通过你的 flake 安装 Mandara。' ;;
        ja:welcome) printf '%s\n' 'あなたの Wayland デスクトップ。準備を始めましょう。' ;;
        ja:signature) printf '%s\n' 'リリースの署名を確認しています…' ;;
        ja:download) printf '%s\n' 'Mandara をダウンロードしています…' ;;
        ja:verify) printf '%s\n' 'ダウンロードを検証しています…' ;;
        ja:unpack) printf '%s\n' 'インストーラーを展開しています…' ;;
        ja:start) printf '%s\n' '準備完了。Mandara のインストーラーを開いています…' ;;
        ja:missing_tool) printf '%s\n' 'このツールをインストールして、もう一度お試しください：' ;;
        ja:unsupported) printf '%s\n' 'ビルド済みリリースには Linux と x86_64 または aarch64 CPU が必要です。' ;;
        ja:https) printf '%s\n' 'リリースミラーは HTTPS を使用する必要があります。' ;;
        ja:temporary) printf '%s\n' '一時ダウンロードフォルダーを作成できませんでした。' ;;
        ja:failed_download) printf '%s\n' 'ダウンロードに失敗しました。接続を確認して、もう一度お試しください。' ;;
        ja:bad_signature) printf '%s\n' 'リリースの署名を検証できませんでした。何もインストールされていません。' ;;
        ja:openssl) printf '%s\n' 'このコマンドには OpenSSL 3 以降が必要です。' ;;
        ja:missing_checksum) printf '%s\n' '署名済みリリースに、このアーカイブの一意のチェックサムがありません。' ;;
        ja:bad_checksum) printf '%s\n' 'ダウンロード内容が署名済みリリースと一致しません。何もインストールされていません。もう一度お試しください。' ;;
        ja:failed_unpack) printf '%s\n' '検証済みリリースを展開できませんでした。' ;;
        ja:missing_installer) printf '%s\n' 'リリース内の実行可能なインストーラーの数が1つではありません。' ;;
        ja:manual) printf '%s\n' '手動ダウンロード：' ;;
        ja:nixos) printf '%s\n' 'NixOS では flake を使って Mandara をインストールしてください。' ;;
        ko:welcome) printf '%s\n' '나만의 Wayland 데스크톱. 함께 준비해 봅시다.' ;;
        ko:signature) printf '%s\n' '릴리스 서명을 확인하는 중…' ;;
        ko:download) printf '%s\n' 'Mandara를 다운로드하는 중…' ;;
        ko:verify) printf '%s\n' '다운로드를 검증하는 중…' ;;
        ko:unpack) printf '%s\n' '설치 프로그램의 압축을 푸는 중…' ;;
        ko:start) printf '%s\n' '준비됐습니다. Mandara 설치 프로그램을 여는 중…' ;;
        ko:missing_tool) printf '%s\n' '이 도구를 설치한 뒤 다시 시도하세요:' ;;
        ko:unsupported) printf '%s\n' '미리 빌드된 릴리스에는 Linux와 x86_64 또는 aarch64 CPU가 필요합니다.' ;;
        ko:https) printf '%s\n' '릴리스 미러는 HTTPS를 사용해야 합니다.' ;;
        ko:temporary) printf '%s\n' '임시 다운로드 폴더를 만들 수 없습니다.' ;;
        ko:failed_download) printf '%s\n' '다운로드에 실패했습니다. 연결을 확인한 뒤 다시 시도하세요.' ;;
        ko:bad_signature) printf '%s\n' '릴리스 서명을 검증할 수 없습니다. 아무것도 설치되지 않았습니다.' ;;
        ko:openssl) printf '%s\n' '이 명령에는 OpenSSL 3 이상이 필요합니다.' ;;
        ko:missing_checksum) printf '%s\n' '서명된 릴리스에 이 압축 파일의 고유 체크섬이 없습니다.' ;;
        ko:bad_checksum) printf '%s\n' '다운로드가 서명된 릴리스와 일치하지 않습니다. 아무것도 설치되지 않았습니다. 다시 시도하세요.' ;;
        ko:failed_unpack) printf '%s\n' '검증된 릴리스의 압축을 풀 수 없습니다.' ;;
        ko:missing_installer) printf '%s\n' '릴리스의 실행 가능한 설치 프로그램 수가 정확히 하나가 아닙니다.' ;;
        ko:manual) printf '%s\n' '직접 다운로드:' ;;
        ko:nixos) printf '%s\n' 'NixOS에서는 flake를 통해 Mandara를 설치하세요.' ;;
        *:welcome) printf '%s\n' 'Your Wayland desktop. Let’s get it ready.' ;;
        *:signature) printf '%s\n' 'Checking the release signature…' ;;
        *:download) printf '%s\n' 'Downloading Mandara…' ;;
        *:verify) printf '%s\n' 'Verifying the download…' ;;
        *:unpack) printf '%s\n' 'Unpacking the installer…' ;;
        *:start) printf '%s\n' 'Ready. Opening Mandara’s installer…' ;;
        *:missing_tool) printf '%s\n' 'Install this tool and try again:' ;;
        *:unsupported) printf '%s\n' 'Prebuilt releases require Linux on an x86_64 or aarch64 CPU.' ;;
        *:https) printf '%s\n' 'The release mirror must use HTTPS.' ;;
        *:temporary) printf '%s\n' 'Could not create a temporary download folder.' ;;
        *:failed_download) printf '%s\n' 'Download failed. Check your connection and try again.' ;;
        *:bad_signature) printf '%s\n' 'The release signature could not be verified. Nothing was installed.' ;;
        *:openssl) printf '%s\n' 'This command needs OpenSSL 3 or newer.' ;;
        *:missing_checksum) printf '%s\n' 'The signed release has no unique checksum for this archive.' ;;
        *:bad_checksum) printf '%s\n' 'The download did not match the signed release. Nothing was installed; please try again.' ;;
        *:failed_unpack) printf '%s\n' 'Could not unpack the verified release.' ;;
        *:missing_installer) printf '%s\n' 'The release does not contain exactly one executable installer.' ;;
        *:manual) printf '%s\n' 'Manual downloads:' ;;
        *:nixos) printf '%s\n' 'On NixOS, install Mandara through your flake.' ;;
    esac
}
# End generated messages

say() { printf '%s\n' "$*" >&2; }
fail() { say "Mandara: $(message "$1")"; shift; if [ "$#" -gt 0 ]; then printf '  %s\n' "$@" >&2; fi; exit 1; }
cleanup() { if [ -n "$work" ]; then rm -rf -- "$work"; fi; }
trap cleanup 0
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 129' HUP

banner() {
    if [ -t 2 ] && [ "${TERM:-dumb}" != dumb ]; then
        cat >&2 <<'PHOENIX'
 ⠈⢷⣤⣀
  ⠰⣝⣻⠿⣷⣶⣤⣀⡀ ⢀⣀⣀
   ⠉⣻⠿⢷⣿⣽⣿⣿⣦⢾⣿⡿⠷⠂
    ⠈⠛⠻⠶⣝⢇⣿⣿⢸⠟ ⢀
       ⣀⣤⣾⠿⠋⠉⢠⣬⣽⣻⣷⡀
     ⣴⡾⣛⡭⣄⡀   ⣸⣿⢿⣿⠃
     ⡟⣾⢹⣇⣿⣽⣶⣽⣛⣋⣓⣫⡕
      ⠻⠞⢿⣾⣿⣿⣽⣿⡿⠛⠁
         ⠈⠉
PHOENIX
    fi
    say 'Mandara'
    say "$(message welcome)"
    say ''
}

fetch() {
    name=$1 destination=$2 limit=$3
    if [ -t 2 ]; then progress=--progress-bar; else progress=--silent; fi
    curl --fail --location --show-error "$progress" \
        --proto "=$protocols" --proto-redir "=$protocols" \
        --retry 3 --connect-timeout 15 --speed-limit 1024 --speed-time 30 \
        --max-time 1800 --max-filesize "$limit" \
        --output "$destination" "$release_base/$name" || fail failed_download "$name"
}

# POSIX printf accepts octal escapes; no xxd, Python or shell hex extension.
hex_bytes() {
    octal=$(printf '%s\n' "$1" | LC_ALL=C awk '
        { for (i = 1; i < length($0); i += 2) {
            a = index("0123456789abcdef", substr($0, i, 1)) - 1
            b = index("0123456789abcdef", substr($0, i + 1, 1)) - 1
            printf "\\0%03o", a * 16 + b
        } }')
    printf '%b' "$octal"
}

check_nixos() {
    if [ -e "$1/etc/NIXOS" ] || [ -e "$1/run/current-system/nixos-version" ]; then
        fail nixos 'https://thehamdiz.github.io/mandara/docs/install.html#nixos'
    fi
}

run_installer() {
    if ( : </dev/tty ) 2>/dev/null; then
        "$@" </dev/tty
    else
        "$@"
    fi
}

download_directory() {
    if [ -n "${TMPDIR:-}" ]; then
        work=$(mktemp -d "$TMPDIR/mandara-install.XXXXXXXX") || fail temporary
        return
    fi
    # /tmp can be noexec; prefer the user's cache for the verified binary.
    base=${XDG_CACHE_HOME:-${HOME:-/tmp}/.cache}
    if mkdir -p -- "$base" 2>/dev/null && work=$(mktemp -d "$base/mandara-install.XXXXXXXX" 2>/dev/null); then
        return
    fi
    work=$(mktemp -d /tmp/mandara-install.XXXXXXXX) || fail temporary
}

main() {
    banner
    check_nixos /
    for program in curl tar openssl sha256sum awk mktemp uname; do
        command -v "$program" >/dev/null 2>&1 || fail missing_tool "$program" "$(message manual) https://github.com/theHamdiz/mandara/releases/latest"
    done
    [ "$(uname -s)" = Linux ] || fail unsupported
    case $(uname -m) in
        x86_64|amd64) arch=x86_64 ;;
        aarch64|arm64) arch=aarch64 ;;
        *) fail unsupported ;;
    esac
    libc=gnu
    if ! { command -v getconf >/dev/null 2>&1 && getconf GNU_LIBC_VERSION >/dev/null 2>&1; }; then
        for loader in /lib/ld-musl-*.so.1 /usr/lib/ld-musl-*.so.1; do
            if [ -e "$loader" ]; then libc=musl; break; fi
        done
    fi
    protocols=https
    case "$release_base" in
        https://*) ;;
        http://127.0.0.1:*) protocols=http ;;
        *) fail https ;;
    esac
    archive="mandara-$arch-linux-$libc.tar.gz"
    download_directory

    say "$(message signature)"
    fetch SHA256SUMS "$work/SHA256SUMS" 1048576
    fetch SHA256SUMS.sig "$work/SHA256SUMS.sig" 1024
    signature=$(LC_ALL=C tr -d '\r\n' < "$work/SHA256SUMS.sig")
    case "$signature" in ''|*[!0-9a-f]*) fail bad_signature ;; esac
    [ "${#signature}" -eq 128 ] || fail bad_signature
    hex_bytes "$signature" > "$work/signature.bin"
    verified=no
    for key in $release_keys; do
        hex_bytes "302a300506032b6570032100$key" > "$work/public.der"
        if openssl pkeyutl -verify -pubin -keyform DER -inkey "$work/public.der" \
            -rawin -in "$work/SHA256SUMS" -sigfile "$work/signature.bin" >/dev/null 2>&1; then
            verified=yes
            break
        fi
    done
    [ "$verified" = yes ] || fail bad_signature "$(message openssl)"
    expected=$(awk -v file="$archive" '$2 == file { print $1 }' "$work/SHA256SUMS")
    case "$expected" in ''|*[!0-9a-f]*) fail missing_checksum "$archive" ;; esac
    [ "${#expected}" -eq 64 ] || fail missing_checksum "$archive"

    say "$(message download) $arch ($libc)"
    fetch "$archive" "$work/$archive" 1073741824
    say "$(message verify)"
    if ! (cd "$work" && printf '%s  %s\n' "$expected" "$archive" | sha256sum -c - >/dev/null); then
        fail bad_checksum
    fi
    say "$(message unpack)"
    tar -xzf "$work/$archive" -C "$work" || fail failed_unpack
    installer=
    for candidate in "$work"/mandara-*/bin/mandara; do
        [ -f "$candidate" ] && [ -x "$candidate" ] || continue
        [ -z "$installer" ] || fail missing_installer
        installer=$candidate
    done
    [ -n "$installer" ] || fail missing_installer
    say ''
    say "$(message start)"
    run_installer "$installer" install "$@"
}

main "$@"
