#!/bin/sh
# Downloads a verified release; Mandara's Rust installer handles the rest.
set -eu
installer_umask=$(umask)
umask 077

release_keys='2fde15bd0518f2352436174a3bcc1e41d62e3ac99647a9dfa773d6462970b43d'
release_base=${MANDARA_BOOTSTRAP_BASE_URL:-https://github.com/theHamdiz/mandara/releases/latest/download}
work=
downloader=curl
language=${LC_ALL:-${LC_MESSAGES:-${LANG:-en}}}
language=${language%%[_.@-]*}

# Generated messages: cargo xtask bootstrap
message() {
    case "$language:$1" in
        ar:welcome) printf '%s\n' 'سطح مكتب Wayland الخاص بك. لنجهزه معًا.' ;;
        ar:signature) printf '%s\n' 'جارٍ التحقق من توقيع الإصدار…' ;;
        ar:signature_ok) printf '%s\n' 'تم التحقق من توقيع الإصدار.' ;;
        ar:download) printf '%s\n' 'جارٍ تنزيل Mandara…' ;;
        ar:download_ok) printf '%s\n' 'تم التنزيل:' ;;
        ar:verify) printf '%s\n' 'جارٍ التحقق من التنزيل…' ;;
        ar:checksum_ok) printf '%s\n' 'تم التحقق من المجموع الاختباري للأرشيف.' ;;
        ar:unpack) printf '%s\n' 'جارٍ فك ملفات المثبّت…' ;;
        ar:start) printf '%s\n' 'كل شيء جاهز. جارٍ فتح مثبّت Mandara…' ;;
        ar:missing_tool) printf '%s\n' 'ثبّت هذه الأداة ثم أعد المحاولة:' ;;
        ar:unsupported) printf '%s\n' 'تتطلب الإصدارات الجاهزة Linux ومعالج x86_64 أو aarch64، أو FreeBSD ومعالج x86_64.' ;;
        ar:https) printf '%s\n' 'يجب أن تستخدم مرآة الإصدارات HTTPS.' ;;
        ar:temporary) printf '%s\n' 'تعذر إنشاء مجلد مؤقت للتنزيل.' ;;
        ar:failed_download) printf '%s\n' 'فشل التنزيل. تحقق من اتصالك وأعد المحاولة.' ;;
        ar:failed_metadata) printf '%s\n' 'تعذر تنزيل الإصدار الموقّع. لم يُثبّت أي شيء. تحقق من اتصالك وأعد المحاولة.' ;;
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
        fa:signature_ok) printf '%s\n' 'امضای انتشار تأیید شد.' ;;
        fa:download) printf '%s\n' 'در حال دریافت Mandara…' ;;
        fa:download_ok) printf '%s\n' 'بارگیری شد:' ;;
        fa:verify) printf '%s\n' 'در حال بررسی فایل دریافتی…' ;;
        fa:checksum_ok) printf '%s\n' 'جمع کنترلی بایگانی تأیید شد.' ;;
        fa:unpack) printf '%s\n' 'در حال استخراج فایل‌های نصب…' ;;
        fa:start) printf '%s\n' 'آماده است. در حال باز کردن نصب‌کنندهٔ Mandara…' ;;
        fa:missing_tool) printf '%s\n' 'این ابزار را نصب و دوباره تلاش کنید:' ;;
        fa:unsupported) printf '%s\n' 'نسخه‌های آماده به Linux با پردازندهٔ x86_64 یا aarch64، یا FreeBSD با پردازندهٔ x86_64 نیاز دارند.' ;;
        fa:https) printf '%s\n' 'آینهٔ انتشار باید از HTTPS استفاده کند.' ;;
        fa:temporary) printf '%s\n' 'ساخت پوشهٔ موقت دریافت ممکن نشد.' ;;
        fa:failed_download) printf '%s\n' 'دریافت ناموفق بود. اتصال خود را بررسی و دوباره تلاش کنید.' ;;
        fa:failed_metadata) printf '%s\n' 'بارگیری انتشار امضاشده ممکن نشد. چیزی نصب نشد. اتصال خود را بررسی کنید و دوباره تلاش کنید.' ;;
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
        ur:signature_ok) printf '%s\n' 'ریلیز کے دستخط کی تصدیق ہو گئی۔' ;;
        ur:download) printf '%s\n' 'Mandara ڈاؤن لوڈ ہو رہا ہے…' ;;
        ur:download_ok) printf '%s\n' 'ڈاؤن لوڈ ہو گیا:' ;;
        ur:verify) printf '%s\n' 'ڈاؤن لوڈ کی تصدیق ہو رہی ہے…' ;;
        ur:checksum_ok) printf '%s\n' 'آرکائیو کے چیک سم کی تصدیق ہو گئی۔' ;;
        ur:unpack) printf '%s\n' 'انسٹالر کی فائلیں نکالی جا رہی ہیں…' ;;
        ur:start) printf '%s\n' 'تیار ہے۔ Mandara انسٹالر کھولا جا رہا ہے…' ;;
        ur:missing_tool) printf '%s\n' 'یہ ٹول نصب کریں اور دوبارہ کوشش کریں:' ;;
        ur:unsupported) printf '%s\n' 'تیار ریلیز کے لیے Linux اور x86_64 یا aarch64 پروسیسر، یا FreeBSD اور x86_64 پروسیسر درکار ہے۔' ;;
        ur:https) printf '%s\n' 'ریلیز مرر کو HTTPS استعمال کرنا چاہیے۔' ;;
        ur:temporary) printf '%s\n' 'ڈاؤن لوڈ کے لیے عارضی فولڈر نہیں بنایا جا سکا۔' ;;
        ur:failed_download) printf '%s\n' 'ڈاؤن لوڈ ناکام ہوا۔ اپنا کنکشن چیک کریں اور دوبارہ کوشش کریں۔' ;;
        ur:failed_metadata) printf '%s\n' 'دستخط شدہ ریلیز ڈاؤن لوڈ نہیں ہو سکی۔ کچھ بھی نصب نہیں ہوا۔ اپنا کنکشن چیک کریں اور دوبارہ کوشش کریں۔' ;;
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
        es:signature_ok) printf '%s\n' 'Firma de la versión verificada.' ;;
        es:download) printf '%s\n' 'Descargando Mandara…' ;;
        es:download_ok) printf '%s\n' 'Descargado:' ;;
        es:verify) printf '%s\n' 'Verificando la descarga…' ;;
        es:checksum_ok) printf '%s\n' 'Suma de comprobación del archivo verificada.' ;;
        es:unpack) printf '%s\n' 'Descomprimiendo el instalador…' ;;
        es:start) printf '%s\n' 'Todo listo. Abriendo el instalador de Mandara…' ;;
        es:missing_tool) printf '%s\n' 'Instala esta herramienta y vuelve a intentarlo:' ;;
        es:unsupported) printf '%s\n' 'Las versiones precompiladas requieren Linux con una CPU x86_64 o aarch64, o FreeBSD con una CPU x86_64.' ;;
        es:https) printf '%s\n' 'El servidor de versiones alternativo debe usar HTTPS.' ;;
        es:temporary) printf '%s\n' 'No se pudo crear una carpeta temporal para la descarga.' ;;
        es:failed_download) printf '%s\n' 'La descarga falló. Comprueba tu conexión y vuelve a intentarlo.' ;;
        es:failed_metadata) printf '%s\n' 'No se pudo descargar la versión firmada. No se instaló nada. Comprueba tu conexión y vuelve a intentarlo.' ;;
        es:bad_signature) printf '%s\n' 'No se pudo verificar la firma de la versión. No se instaló nada.' ;;
        es:openssl) printf '%s\n' 'Este comando necesita OpenSSL 3 o posterior.' ;;
        es:missing_checksum) printf '%s\n' 'La versión firmada no tiene una suma de verificación única para este archivo.' ;;
        es:bad_checksum) printf '%s\n' 'La descarga no coincide con la versión firmada. No se instaló nada; vuelve a intentarlo.' ;;
        es:failed_unpack) printf '%s\n' 'No se pudo descomprimir la versión verificada.' ;;
        es:missing_installer) printf '%s\n' 'La versión no contiene exactamente un instalador ejecutable.' ;;
        es:manual) printf '%s\n' 'Descargas manuales:' ;;
        es:nixos) printf '%s\n' 'En NixOS, instala Mandara mediante tu flake.' ;;
        pt:welcome) printf '%s\n' 'Sua área de trabalho Wayland. Vamos prepará-la.' ;;
        pt:signature) printf '%s\n' 'Verificando a assinatura da versão…' ;;
        pt:signature_ok) printf '%s\n' 'Assinatura da versão verificada.' ;;
        pt:download) printf '%s\n' 'Baixando o Mandara…' ;;
        pt:download_ok) printf '%s\n' 'Baixado:' ;;
        pt:verify) printf '%s\n' 'Verificando o download…' ;;
        pt:checksum_ok) printf '%s\n' 'Soma de verificação do arquivo confirmada.' ;;
        pt:unpack) printf '%s\n' 'Extraindo o instalador…' ;;
        pt:start) printf '%s\n' 'Pronto. Abrindo o instalador do Mandara…' ;;
        pt:missing_tool) printf '%s\n' 'Instale esta ferramenta e tente novamente:' ;;
        pt:unsupported) printf '%s\n' 'As versões pré-compiladas exigem Linux em uma CPU x86_64 ou aarch64, ou FreeBSD em uma CPU x86_64.' ;;
        pt:https) printf '%s\n' 'O espelho de versões deve usar HTTPS.' ;;
        pt:temporary) printf '%s\n' 'Não foi possível criar uma pasta temporária para o download.' ;;
        pt:failed_download) printf '%s\n' 'O download falhou. Confira sua conexão e tente novamente.' ;;
        pt:failed_metadata) printf '%s\n' 'Não foi possível baixar a versão assinada. Nada foi instalado. Verifique sua conexão e tente novamente.' ;;
        pt:bad_signature) printf '%s\n' 'Não foi possível verificar a assinatura da versão. Nada foi instalado.' ;;
        pt:openssl) printf '%s\n' 'Este comando exige OpenSSL 3 ou mais recente.' ;;
        pt:missing_checksum) printf '%s\n' 'A versão assinada não tem uma soma de verificação única para este arquivo.' ;;
        pt:bad_checksum) printf '%s\n' 'O download não corresponde à versão assinada. Nada foi instalado; tente novamente.' ;;
        pt:failed_unpack) printf '%s\n' 'Não foi possível extrair a versão verificada.' ;;
        pt:missing_installer) printf '%s\n' 'A versão não contém exatamente um instalador executável.' ;;
        pt:manual) printf '%s\n' 'Downloads manuais:' ;;
        pt:nixos) printf '%s\n' 'No NixOS, instale o Mandara pelo seu flake.' ;;
        fr:welcome) printf '%s\n' 'Votre bureau Wayland. Préparons-le ensemble.' ;;
        fr:signature) printf '%s\n' 'Vérification de la signature de la version…' ;;
        fr:signature_ok) printf '%s\n' 'Signature de la version vérifiée.' ;;
        fr:download) printf '%s\n' 'Téléchargement de Mandara…' ;;
        fr:download_ok) printf '%s\n' 'Téléchargé :' ;;
        fr:verify) printf '%s\n' 'Vérification du téléchargement…' ;;
        fr:checksum_ok) printf '%s\n' 'Somme de contrôle de l’archive vérifiée.' ;;
        fr:unpack) printf '%s\n' 'Décompression du programme d’installation…' ;;
        fr:start) printf '%s\n' 'Tout est prêt. Ouverture du programme d’installation de Mandara…' ;;
        fr:missing_tool) printf '%s\n' 'Installez cet outil, puis réessayez :' ;;
        fr:unsupported) printf '%s\n' 'Les versions précompilées nécessitent Linux et un processeur x86_64 ou aarch64, ou FreeBSD et un processeur x86_64.' ;;
        fr:https) printf '%s\n' 'Le miroir des versions doit utiliser HTTPS.' ;;
        fr:temporary) printf '%s\n' 'Impossible de créer un dossier temporaire pour le téléchargement.' ;;
        fr:failed_download) printf '%s\n' 'Le téléchargement a échoué. Vérifiez votre connexion et réessayez.' ;;
        fr:failed_metadata) printf '%s\n' 'Impossible de télécharger la version signée. Rien n’a été installé. Vérifiez votre connexion et réessayez.' ;;
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
        de:signature_ok) printf '%s\n' 'Signatur der Veröffentlichung bestätigt.' ;;
        de:download) printf '%s\n' 'Mandara wird heruntergeladen…' ;;
        de:download_ok) printf '%s\n' 'Heruntergeladen:' ;;
        de:verify) printf '%s\n' 'Download wird geprüft…' ;;
        de:checksum_ok) printf '%s\n' 'Prüfsumme des Archivs bestätigt.' ;;
        de:unpack) printf '%s\n' 'Installationsprogramm wird entpackt…' ;;
        de:start) printf '%s\n' 'Alles bereit. Das Mandara-Installationsprogramm wird geöffnet…' ;;
        de:missing_tool) printf '%s\n' 'Installiere dieses Werkzeug und versuche es erneut:' ;;
        de:unsupported) printf '%s\n' 'Vorgefertigte Versionen benötigen Linux mit einer x86_64- oder aarch64-CPU oder FreeBSD mit einer x86_64-CPU.' ;;
        de:https) printf '%s\n' 'Der Download-Spiegel muss HTTPS verwenden.' ;;
        de:temporary) printf '%s\n' 'Ein temporärer Download-Ordner konnte nicht erstellt werden.' ;;
        de:failed_download) printf '%s\n' 'Download fehlgeschlagen. Prüfe deine Verbindung und versuche es erneut.' ;;
        de:failed_metadata) printf '%s\n' 'Die signierte Veröffentlichung konnte nicht heruntergeladen werden. Es wurde nichts installiert. Prüfe deine Verbindung und versuche es erneut.' ;;
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
        tr:signature_ok) printf '%s\n' 'Sürüm imzası doğrulandı.' ;;
        tr:download) printf '%s\n' 'Mandara indiriliyor…' ;;
        tr:download_ok) printf '%s\n' 'İndirildi:' ;;
        tr:verify) printf '%s\n' 'İndirme doğrulanıyor…' ;;
        tr:checksum_ok) printf '%s\n' 'Arşiv sağlama toplamı doğrulandı.' ;;
        tr:unpack) printf '%s\n' 'Kurulum dosyaları arşivden çıkarılıyor…' ;;
        tr:start) printf '%s\n' 'Hazır. Mandara kurulum programı açılıyor…' ;;
        tr:missing_tool) printf '%s\n' 'Bu aracı yükleyip yeniden deneyin:' ;;
        tr:unsupported) printf '%s\n' 'Hazır sürümler için Linux ve x86_64 veya aarch64 işlemci ya da FreeBSD ve x86_64 işlemci gerekir.' ;;
        tr:https) printf '%s\n' 'Sürüm aynası HTTPS kullanmalıdır.' ;;
        tr:temporary) printf '%s\n' 'Geçici indirme klasörü oluşturulamadı.' ;;
        tr:failed_download) printf '%s\n' 'İndirme başarısız oldu. Bağlantınızı denetleyip yeniden deneyin.' ;;
        tr:failed_metadata) printf '%s\n' 'İmzalı sürüm indirilemedi. Hiçbir şey kurulmadı. Bağlantınızı kontrol edip yeniden deneyin.' ;;
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
        ru:signature_ok) printf '%s\n' 'Подпись выпуска проверена.' ;;
        ru:download) printf '%s\n' 'Загрузка Mandara…' ;;
        ru:download_ok) printf '%s\n' 'Загружено:' ;;
        ru:verify) printf '%s\n' 'Проверка загрузки…' ;;
        ru:checksum_ok) printf '%s\n' 'Контрольная сумма архива проверена.' ;;
        ru:unpack) printf '%s\n' 'Распаковка установщика…' ;;
        ru:start) printf '%s\n' 'Всё готово. Запуск установщика Mandara…' ;;
        ru:missing_tool) printf '%s\n' 'Установите этот инструмент и повторите попытку:' ;;
        ru:unsupported) printf '%s\n' 'Готовые сборки требуют Linux и процессор x86_64 или aarch64 либо FreeBSD и процессор x86_64.' ;;
        ru:https) printf '%s\n' 'Зеркало выпусков должно использовать HTTPS.' ;;
        ru:temporary) printf '%s\n' 'Не удалось создать временную папку для загрузки.' ;;
        ru:failed_download) printf '%s\n' 'Загрузка не удалась. Проверьте соединение и повторите попытку.' ;;
        ru:failed_metadata) printf '%s\n' 'Не удалось загрузить подписанный выпуск. Ничего не установлено. Проверьте соединение и попробуйте снова.' ;;
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
        hi:signature_ok) printf '%s\n' 'रिलीज़ का हस्ताक्षर सत्यापित है।' ;;
        hi:download) printf '%s\n' 'Mandara डाउनलोड हो रहा है…' ;;
        hi:download_ok) printf '%s\n' 'डाउनलोड किया गया:' ;;
        hi:verify) printf '%s\n' 'डाउनलोड की जाँच हो रही है…' ;;
        hi:checksum_ok) printf '%s\n' 'आर्काइव का चेकसम सत्यापित है।' ;;
        hi:unpack) printf '%s\n' 'इंस्टॉलर की फ़ाइलें निकाली जा रही हैं…' ;;
        hi:start) printf '%s\n' 'तैयार है। Mandara इंस्टॉलर खुल रहा है…' ;;
        hi:missing_tool) printf '%s\n' 'यह टूल इंस्टॉल करें और फिर कोशिश करें:' ;;
        hi:unsupported) printf '%s\n' 'तैयार रिलीज़ के लिए Linux और x86_64 या aarch64 CPU, या FreeBSD और x86_64 CPU चाहिए।' ;;
        hi:https) printf '%s\n' 'रिलीज़ मिरर को HTTPS इस्तेमाल करना चाहिए।' ;;
        hi:temporary) printf '%s\n' 'अस्थायी डाउनलोड फ़ोल्डर नहीं बनाया जा सका।' ;;
        hi:failed_download) printf '%s\n' 'डाउनलोड विफल रहा। अपना कनेक्शन जाँचें और फिर कोशिश करें।' ;;
        hi:failed_metadata) printf '%s\n' 'हस्ताक्षरित रिलीज़ डाउनलोड नहीं हो सकी। कुछ भी इंस्टॉल नहीं हुआ। अपना कनेक्शन जाँचें और फिर कोशिश करें।' ;;
        hi:bad_signature) printf '%s\n' 'रिलीज़ के हस्ताक्षर की पुष्टि नहीं हो सकी। कुछ भी इंस्टॉल नहीं हुआ।' ;;
        hi:openssl) printf '%s\n' 'इस कमांड के लिए OpenSSL 3 या नया संस्करण चाहिए।' ;;
        hi:missing_checksum) printf '%s\n' 'हस्ताक्षरित रिलीज़ में इस आर्काइव के लिए ठीक एक चेकसम नहीं मिला।' ;;
        hi:bad_checksum) printf '%s\n' 'डाउनलोड हस्ताक्षरित रिलीज़ से मेल नहीं खाता। कुछ भी इंस्टॉल नहीं हुआ; फिर कोशिश करें।' ;;
        hi:failed_unpack) printf '%s\n' 'सत्यापित रिलीज़ की फ़ाइलें नहीं निकाली जा सकीं।' ;;
        hi:missing_installer) printf '%s\n' 'रिलीज़ में चलने योग्य इंस्टॉलर की संख्या ठीक एक नहीं है।' ;;
        hi:manual) printf '%s\n' 'मैन्युअल डाउनलोड:' ;;
        hi:nixos) printf '%s\n' 'NixOS पर अपनी flake से Mandara इंस्टॉल करें।' ;;
        mr:welcome) printf '%s\n' 'तुमचा Wayland डेस्कटॉप. चला, तो तयार करूया.' ;;
        mr:signature) printf '%s\n' 'प्रकाशनाची स्वाक्षरी तपासत आहे…' ;;
        mr:signature_ok) printf '%s\n' 'प्रकाशनाची स्वाक्षरी पडताळली.' ;;
        mr:download) printf '%s\n' 'Mandara डाउनलोड करत आहे…' ;;
        mr:download_ok) printf '%s\n' 'डाउनलोड केले:' ;;
        mr:verify) printf '%s\n' 'डाउनलोड पडताळत आहे…' ;;
        mr:checksum_ok) printf '%s\n' 'संग्रहाची चेकसम पडताळली.' ;;
        mr:unpack) printf '%s\n' 'इंस्टॉलर उलगडत आहे…' ;;
        mr:start) printf '%s\n' 'तयार. Mandara चे इंस्टॉलर उघडत आहे…' ;;
        mr:missing_tool) printf '%s\n' 'हे साधन बसवा आणि पुन्हा प्रयत्न करा:' ;;
        mr:unsupported) printf '%s\n' 'आधी तयार केलेल्या आवृत्त्यांसाठी x86_64 किंवा aarch64 CPU वर Linux, किंवा x86_64 CPU वर FreeBSD आवश्यक आहे.' ;;
        mr:https) printf '%s\n' 'प्रकाशनाच्या पर्यायी सर्व्हरने HTTPS वापरणे आवश्यक आहे.' ;;
        mr:temporary) printf '%s\n' 'डाउनलोडसाठी तात्पुरते फोल्डर तयार करता आले नाही.' ;;
        mr:failed_download) printf '%s\n' 'डाउनलोड अयशस्वी. जोडणी तपासा आणि पुन्हा प्रयत्न करा.' ;;
        mr:failed_metadata) printf '%s\n' 'स्वाक्षरी केलेले प्रकाशन डाउनलोड करता आले नाही. काहीही स्थापित केलेले नाही. तुमची जोडणी तपासा आणि पुन्हा प्रयत्न करा.' ;;
        mr:bad_signature) printf '%s\n' 'प्रकाशनाची स्वाक्षरी पडताळता आली नाही. काहीही बसवले नाही.' ;;
        mr:openssl) printf '%s\n' 'या आदेशाला OpenSSL 3 किंवा नवीन आवृत्ती आवश्यक आहे.' ;;
        mr:missing_checksum) printf '%s\n' 'स्वाक्षरी केलेल्या प्रकाशनात या संग्रहासाठी एकमेव पडताळणी बेरीज नाही.' ;;
        mr:bad_checksum) printf '%s\n' 'डाउनलोड स्वाक्षरी केलेल्या प्रकाशनाशी जुळले नाही. काहीही बसवले नाही; कृपया पुन्हा प्रयत्न करा.' ;;
        mr:failed_unpack) printf '%s\n' 'तपासलेल्या प्रकाशनाची फाइल उलगडता आली नाही.' ;;
        mr:missing_installer) printf '%s\n' 'प्रकाशनात चालवता येईल असे नेमके एक इंस्टॉलर नाही.' ;;
        mr:manual) printf '%s\n' 'स्वहस्ते डाउनलोड:' ;;
        mr:nixos) printf '%s\n' 'NixOS वर तुमच्या flake मधून Mandara बसवा.' ;;
        bn:welcome) printf '%s\n' 'আপনার Wayland ডেস্কটপ। চলুন প্রস্তুত করি।' ;;
        bn:signature) printf '%s\n' 'রিলিজের স্বাক্ষর যাচাই করা হচ্ছে…' ;;
        bn:signature_ok) printf '%s\n' 'রিলিজের স্বাক্ষর যাচাই করা হয়েছে।' ;;
        bn:download) printf '%s\n' 'Mandara ডাউনলোড করা হচ্ছে…' ;;
        bn:download_ok) printf '%s\n' 'ডাউনলোড হয়েছে:' ;;
        bn:verify) printf '%s\n' 'ডাউনলোড যাচাই করা হচ্ছে…' ;;
        bn:checksum_ok) printf '%s\n' 'আর্কাইভের চেকসাম যাচাই করা হয়েছে।' ;;
        bn:unpack) printf '%s\n' 'ইনস্টলারের ফাইল আনপ্যাক করা হচ্ছে…' ;;
        bn:start) printf '%s\n' 'প্রস্তুত। Mandara ইনস্টলার খোলা হচ্ছে…' ;;
        bn:missing_tool) printf '%s\n' 'এই টুলটি ইনস্টল করে আবার চেষ্টা করুন:' ;;
        bn:unsupported) printf '%s\n' 'আগে থেকে তৈরি রিলিজের জন্য Linux এবং x86_64 বা aarch64 CPU, অথবা FreeBSD এবং x86_64 CPU প্রয়োজন।' ;;
        bn:https) printf '%s\n' 'রিলিজ মিররে HTTPS ব্যবহার করতে হবে।' ;;
        bn:temporary) printf '%s\n' 'ডাউনলোডের জন্য অস্থায়ী ফোল্ডার তৈরি করা যায়নি।' ;;
        bn:failed_download) printf '%s\n' 'ডাউনলোড ব্যর্থ হয়েছে। সংযোগ পরীক্ষা করে আবার চেষ্টা করুন।' ;;
        bn:failed_metadata) printf '%s\n' 'স্বাক্ষরিত রিলিজ ডাউনলোড করা যায়নি। কিছুই ইনস্টল করা হয়নি। সংযোগ পরীক্ষা করে আবার চেষ্টা করুন।' ;;
        bn:bad_signature) printf '%s\n' 'রিলিজের স্বাক্ষর যাচাই করা যায়নি। কিছুই ইনস্টল করা হয়নি।' ;;
        bn:openssl) printf '%s\n' 'এই কমান্ডের জন্য OpenSSL 3 বা পরবর্তী সংস্করণ প্রয়োজন।' ;;
        bn:missing_checksum) printf '%s\n' 'স্বাক্ষরিত রিলিজে এই আর্কাইভের জন্য একটিমাত্র চেকসাম নেই।' ;;
        bn:bad_checksum) printf '%s\n' 'ডাউনলোডটি স্বাক্ষরিত রিলিজের সঙ্গে মেলেনি। কিছুই ইনস্টল করা হয়নি; আবার চেষ্টা করুন।' ;;
        bn:failed_unpack) printf '%s\n' 'যাচাই করা রিলিজের ফাইল আনপ্যাক করা যায়নি।' ;;
        bn:missing_installer) printf '%s\n' 'রিলিজে চালানো যায় এমন ইনস্টলারের সংখ্যা ঠিক এক নয়।' ;;
        bn:manual) printf '%s\n' 'নিজে ডাউনলোড করুন:' ;;
        bn:nixos) printf '%s\n' 'NixOS-এ আপনার flake দিয়ে Mandara ইনস্টল করুন।' ;;
        te:welcome) printf '%s\n' 'మీ Wayland డెస్క్‌టాప్. దాన్ని సిద్ధం చేద్దాం.' ;;
        te:signature) printf '%s\n' 'విడుదల సంతకాన్ని తనిఖీ చేస్తోంది…' ;;
        te:signature_ok) printf '%s\n' 'విడుదల సంతకం ధృవీకరించబడింది.' ;;
        te:download) printf '%s\n' 'Mandara ను డౌన్‌లోడ్ చేస్తోంది…' ;;
        te:download_ok) printf '%s\n' 'డౌన్‌లోడ్ అయింది:' ;;
        te:verify) printf '%s\n' 'డౌన్‌లోడ్‌ను ధృవీకరిస్తోంది…' ;;
        te:checksum_ok) printf '%s\n' 'ఆర్కైవ్ చెక్‌సమ్ ధృవీకరించబడింది.' ;;
        te:unpack) printf '%s\n' 'ఇన్‌స్టాలర్‌ను అన్‌ప్యాక్ చేస్తోంది…' ;;
        te:start) printf '%s\n' 'సిద్ధంగా ఉంది. Mandara ఇన్‌స్టాలర్‌ను తెరుస్తోంది…' ;;
        te:missing_tool) printf '%s\n' 'ఈ సాధనాన్ని ఇన్‌స్టాల్ చేసి మళ్లీ ప్రయత్నించండి:' ;;
        te:unsupported) printf '%s\n' 'ముందుగా నిర్మించిన విడుదలలకు x86_64 లేదా aarch64 CPU పై Linux, లేదా x86_64 CPU పై FreeBSD అవసరం.' ;;
        te:https) printf '%s\n' 'విడుదల మిర్రర్ తప్పనిసరిగా HTTPS ఉపయోగించాలి.' ;;
        te:temporary) printf '%s\n' 'డౌన్‌లోడ్ కోసం తాత్కాలిక ఫోల్డర్‌ను సృష్టించలేకపోయింది.' ;;
        te:failed_download) printf '%s\n' 'డౌన్‌లోడ్ విఫలమైంది. మీ కనెక్షన్‌ను తనిఖీ చేసి మళ్లీ ప్రయత్నించండి.' ;;
        te:failed_metadata) printf '%s\n' 'సంతకం చేసిన విడుదలను డౌన్‌లోడ్ చేయలేకపోయాము. ఏదీ ఇన్‌స్టాల్ కాలేదు. మీ కనెక్షన్‌ను తనిఖీ చేసి మళ్లీ ప్రయత్నించండి.' ;;
        te:bad_signature) printf '%s\n' 'విడుదల సంతకాన్ని ధృవీకరించలేకపోయింది. ఏదీ ఇన్‌స్టాల్ కాలేదు.' ;;
        te:openssl) printf '%s\n' 'ఈ ఆదేశానికి OpenSSL 3 లేదా కొత్తది అవసరం.' ;;
        te:missing_checksum) printf '%s\n' 'సంతకం చేసిన విడుదలలో ఈ ఆర్కైవ్‌కు ప్రత్యేకమైన ఒక్క చెక్‌సమ్ లేదు.' ;;
        te:bad_checksum) printf '%s\n' 'డౌన్‌లోడ్ సంతకం చేసిన విడుదలతో సరిపోలలేదు. ఏదీ ఇన్‌స్టాల్ కాలేదు; దయచేసి మళ్లీ ప్రయత్నించండి.' ;;
        te:failed_unpack) printf '%s\n' 'ధృవీకరించిన విడుదలను అన్‌ప్యాక్ చేయలేకపోయింది.' ;;
        te:missing_installer) printf '%s\n' 'విడుదలలో అమలు చేయగల ఇన్‌స్టాలర్ ఖచ్చితంగా ఒకటి లేదు.' ;;
        te:manual) printf '%s\n' 'మానవీయ డౌన్‌లోడ్‌లు:' ;;
        te:nixos) printf '%s\n' 'NixOS లో మీ flake ద్వారా Mandara ను ఇన్‌స్టాల్ చేయండి.' ;;
        id:welcome) printf '%s\n' 'Desktop Wayland Anda. Mari kita siapkan.' ;;
        id:signature) printf '%s\n' 'Memeriksa tanda tangan rilis…' ;;
        id:signature_ok) printf '%s\n' 'Tanda tangan rilis terverifikasi.' ;;
        id:download) printf '%s\n' 'Mengunduh Mandara…' ;;
        id:download_ok) printf '%s\n' 'Diunduh:' ;;
        id:verify) printf '%s\n' 'Memverifikasi unduhan…' ;;
        id:checksum_ok) printf '%s\n' 'Checksum arsip terverifikasi.' ;;
        id:unpack) printf '%s\n' 'Mengekstrak pemasang…' ;;
        id:start) printf '%s\n' 'Siap. Membuka pemasang Mandara…' ;;
        id:missing_tool) printf '%s\n' 'Pasang alat ini lalu coba lagi:' ;;
        id:unsupported) printf '%s\n' 'Rilis siap pakai memerlukan Linux dengan CPU x86_64 atau aarch64, atau FreeBSD dengan CPU x86_64.' ;;
        id:https) printf '%s\n' 'Cermin rilis harus menggunakan HTTPS.' ;;
        id:temporary) printf '%s\n' 'Tidak dapat membuat folder unduhan sementara.' ;;
        id:failed_download) printf '%s\n' 'Unduhan gagal. Periksa koneksi Anda dan coba lagi.' ;;
        id:failed_metadata) printf '%s\n' 'Rilis bertanda tangan tidak dapat diunduh. Tidak ada yang dipasang. Periksa koneksi Anda dan coba lagi.' ;;
        id:bad_signature) printf '%s\n' 'Tanda tangan rilis tidak dapat diverifikasi. Tidak ada yang dipasang.' ;;
        id:openssl) printf '%s\n' 'Perintah ini memerlukan OpenSSL 3 atau yang lebih baru.' ;;
        id:missing_checksum) printf '%s\n' 'Rilis bertanda tangan tidak memiliki satu checksum unik untuk arsip ini.' ;;
        id:bad_checksum) printf '%s\n' 'Unduhan tidak cocok dengan rilis bertanda tangan. Tidak ada yang dipasang; silakan coba lagi.' ;;
        id:failed_unpack) printf '%s\n' 'Tidak dapat mengekstrak rilis yang telah diverifikasi.' ;;
        id:missing_installer) printf '%s\n' 'Rilis tidak berisi tepat satu pemasang yang dapat dijalankan.' ;;
        id:manual) printf '%s\n' 'Unduhan manual:' ;;
        id:nixos) printf '%s\n' 'Di NixOS, pasang Mandara melalui flake Anda.' ;;
        sw:welcome) printf '%s\n' 'Eneo-kazi lako la Wayland. Hebu tuliandae.' ;;
        sw:signature) printf '%s\n' 'Inakagua sahihi ya toleo…' ;;
        sw:signature_ok) printf '%s\n' 'Sahihi ya toleo imethibitishwa.' ;;
        sw:download) printf '%s\n' 'Inapakua Mandara…' ;;
        sw:download_ok) printf '%s\n' 'Imepakuliwa:' ;;
        sw:verify) printf '%s\n' 'Inathibitisha upakuaji…' ;;
        sw:checksum_ok) printf '%s\n' 'Jumla ya ukaguzi ya jalada imethibitishwa.' ;;
        sw:unpack) printf '%s\n' 'Inafungua kifurushi cha kisakinishi…' ;;
        sw:start) printf '%s\n' 'Tayari. Inafungua kisakinishi cha Mandara…' ;;
        sw:missing_tool) printf '%s\n' 'Sakinisha zana hii kisha ujaribu tena:' ;;
        sw:unsupported) printf '%s\n' 'Matoleo yaliyotengenezwa tayari yanahitaji Linux kwenye CPU ya x86_64 au aarch64, au FreeBSD kwenye CPU ya x86_64.' ;;
        sw:https) printf '%s\n' 'Seva mbadala ya matoleo lazima itumie HTTPS.' ;;
        sw:temporary) printf '%s\n' 'Haikuweza kuunda folda ya muda ya upakuaji.' ;;
        sw:failed_download) printf '%s\n' 'Upakuaji umeshindwa. Kagua muunganisho wako kisha ujaribu tena.' ;;
        sw:failed_metadata) printf '%s\n' 'Imeshindikana kupakua toleo lililosainiwa. Hakuna kilichosakinishwa. Angalia muunganisho wako na ujaribu tena.' ;;
        sw:bad_signature) printf '%s\n' 'Sahihi ya toleo haikuweza kuthibitishwa. Hakuna kilichosakinishwa.' ;;
        sw:openssl) printf '%s\n' 'Amri hii inahitaji OpenSSL 3 au mpya zaidi.' ;;
        sw:missing_checksum) printf '%s\n' 'Toleo lililotiwa sahihi halina jumla moja tu ya ukaguzi kwa kifurushi hiki.' ;;
        sw:bad_checksum) printf '%s\n' 'Upakuaji haulingani na toleo lililotiwa sahihi. Hakuna kilichosakinishwa; tafadhali jaribu tena.' ;;
        sw:failed_unpack) printf '%s\n' 'Haikuweza kufungua kifurushi cha toleo lililothibitishwa.' ;;
        sw:missing_installer) printf '%s\n' 'Toleo halina kisakinishi kimoja tu kinachoweza kutekelezwa.' ;;
        sw:manual) printf '%s\n' 'Upakuaji wa mwenyewe:' ;;
        sw:nixos) printf '%s\n' 'Katika NixOS, sakinisha Mandara kupitia flake yako.' ;;
        vi:welcome) printf '%s\n' 'Màn hình nền Wayland của bạn. Cùng chuẩn bị nhé.' ;;
        vi:signature) printf '%s\n' 'Đang kiểm tra chữ ký bản phát hành…' ;;
        vi:signature_ok) printf '%s\n' 'Đã xác minh chữ ký bản phát hành.' ;;
        vi:download) printf '%s\n' 'Đang tải Mandara…' ;;
        vi:download_ok) printf '%s\n' 'Đã tải xuống:' ;;
        vi:verify) printf '%s\n' 'Đang xác minh tệp tải xuống…' ;;
        vi:checksum_ok) printf '%s\n' 'Đã xác minh tổng kiểm tra của tệp nén.' ;;
        vi:unpack) printf '%s\n' 'Đang giải nén trình cài…' ;;
        vi:start) printf '%s\n' 'Sẵn sàng. Đang mở trình cài Mandara…' ;;
        vi:missing_tool) printf '%s\n' 'Cài công cụ này rồi thử lại:' ;;
        vi:unsupported) printf '%s\n' 'Bản biên dịch sẵn yêu cầu Linux trên CPU x86_64 hoặc aarch64, hoặc FreeBSD trên CPU x86_64.' ;;
        vi:https) printf '%s\n' 'Máy chủ phản chiếu bản phát hành phải dùng HTTPS.' ;;
        vi:temporary) printf '%s\n' 'Không tạo được thư mục tải xuống tạm thời.' ;;
        vi:failed_download) printf '%s\n' 'Tải xuống thất bại. Hãy kiểm tra kết nối và thử lại.' ;;
        vi:failed_metadata) printf '%s\n' 'Không thể tải xuống bản phát hành có chữ ký. Chưa cài đặt gì. Hãy kiểm tra kết nối rồi thử lại.' ;;
        vi:bad_signature) printf '%s\n' 'Không xác minh được chữ ký bản phát hành. Chưa cài đặt gì.' ;;
        vi:openssl) printf '%s\n' 'Lệnh này yêu cầu OpenSSL 3 trở lên.' ;;
        vi:missing_checksum) printf '%s\n' 'Bản phát hành đã ký không có tổng kiểm duy nhất cho kho lưu trữ này.' ;;
        vi:bad_checksum) printf '%s\n' 'Tệp tải xuống không khớp bản phát hành đã ký. Chưa cài đặt gì; vui lòng thử lại.' ;;
        vi:failed_unpack) printf '%s\n' 'Không giải nén được bản phát hành đã xác minh.' ;;
        vi:missing_installer) printf '%s\n' 'Bản phát hành không chứa đúng một trình cài có thể thực thi.' ;;
        vi:manual) printf '%s\n' 'Tải xuống thủ công:' ;;
        vi:nixos) printf '%s\n' 'Trên NixOS, hãy cài Mandara bằng flake của bạn.' ;;
        zh:welcome) printf '%s\n' '你的 Wayland 桌面。让我们准备就绪。' ;;
        zh:signature) printf '%s\n' '正在检查发布版本签名…' ;;
        zh:signature_ok) printf '%s\n' '已验证发行版签名。' ;;
        zh:download) printf '%s\n' '正在下载 Mandara…' ;;
        zh:download_ok) printf '%s\n' '已下载：' ;;
        zh:verify) printf '%s\n' '正在验证下载…' ;;
        zh:checksum_ok) printf '%s\n' '已验证归档文件校验和。' ;;
        zh:unpack) printf '%s\n' '正在解压安装程序…' ;;
        zh:start) printf '%s\n' '准备就绪。正在打开 Mandara 安装程序…' ;;
        zh:missing_tool) printf '%s\n' '请安装此工具后重试：' ;;
        zh:unsupported) printf '%s\n' '预编译版本需要 Linux 系统和 x86_64 或 aarch64 处理器，或 FreeBSD 系统和 x86_64 处理器。' ;;
        zh:https) printf '%s\n' '发布镜像必须使用 HTTPS。' ;;
        zh:temporary) printf '%s\n' '无法创建临时下载文件夹。' ;;
        zh:failed_download) printf '%s\n' '下载失败。请检查网络连接后重试。' ;;
        zh:failed_metadata) printf '%s\n' '无法下载已签名的发行版。未安装任何内容。请检查连接并重试。' ;;
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
        ja:signature_ok) printf '%s\n' 'リリースの署名を確認しました。' ;;
        ja:download) printf '%s\n' 'Mandara をダウンロードしています…' ;;
        ja:download_ok) printf '%s\n' 'ダウンロード済み:' ;;
        ja:verify) printf '%s\n' 'ダウンロードを検証しています…' ;;
        ja:checksum_ok) printf '%s\n' 'アーカイブのチェックサムを確認しました。' ;;
        ja:unpack) printf '%s\n' 'インストーラーを展開しています…' ;;
        ja:start) printf '%s\n' '準備完了。Mandara のインストーラーを開いています…' ;;
        ja:missing_tool) printf '%s\n' 'このツールをインストールして、もう一度お試しください：' ;;
        ja:unsupported) printf '%s\n' 'ビルド済みリリースには、Linux と x86_64 または aarch64 CPU、あるいは FreeBSD と x86_64 CPU が必要です。' ;;
        ja:https) printf '%s\n' 'リリースミラーは HTTPS を使用する必要があります。' ;;
        ja:temporary) printf '%s\n' '一時ダウンロードフォルダーを作成できませんでした。' ;;
        ja:failed_download) printf '%s\n' 'ダウンロードに失敗しました。接続を確認して、もう一度お試しください。' ;;
        ja:failed_metadata) printf '%s\n' '署名付きリリースをダウンロードできませんでした。何もインストールしていません。接続を確認して、もう一度お試しください。' ;;
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
        ko:signature_ok) printf '%s\n' '릴리스 서명이 확인되었습니다.' ;;
        ko:download) printf '%s\n' 'Mandara를 다운로드하는 중…' ;;
        ko:download_ok) printf '%s\n' '다운로드 완료:' ;;
        ko:verify) printf '%s\n' '다운로드를 검증하는 중…' ;;
        ko:checksum_ok) printf '%s\n' '압축 파일의 체크섬이 확인되었습니다.' ;;
        ko:unpack) printf '%s\n' '설치 프로그램의 압축을 푸는 중…' ;;
        ko:start) printf '%s\n' '준비됐습니다. Mandara 설치 프로그램을 여는 중…' ;;
        ko:missing_tool) printf '%s\n' '이 도구를 설치한 뒤 다시 시도하세요:' ;;
        ko:unsupported) printf '%s\n' '미리 빌드된 릴리스에는 Linux와 x86_64 또는 aarch64 CPU, 또는 FreeBSD와 x86_64 CPU가 필요합니다.' ;;
        ko:https) printf '%s\n' '릴리스 미러는 HTTPS를 사용해야 합니다.' ;;
        ko:temporary) printf '%s\n' '임시 다운로드 폴더를 만들 수 없습니다.' ;;
        ko:failed_download) printf '%s\n' '다운로드에 실패했습니다. 연결을 확인한 뒤 다시 시도하세요.' ;;
        ko:failed_metadata) printf '%s\n' '서명된 릴리스를 다운로드하지 못했습니다. 아무것도 설치되지 않았습니다. 연결을 확인하고 다시 시도하세요.' ;;
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
        *:signature_ok) printf '%s\n' 'Release signature verified.' ;;
        *:download) printf '%s\n' 'Downloading Mandara…' ;;
        *:download_ok) printf '%s\n' 'Downloaded:' ;;
        *:verify) printf '%s\n' 'Verifying the download…' ;;
        *:checksum_ok) printf '%s\n' 'Archive checksum verified.' ;;
        *:unpack) printf '%s\n' 'Unpacking the installer…' ;;
        *:start) printf '%s\n' 'Ready. Opening Mandara’s installer…' ;;
        *:missing_tool) printf '%s\n' 'Install this tool and try again:' ;;
        *:unsupported) printf '%s\n' 'Prebuilt releases require Linux on an x86_64 or aarch64 CPU, or FreeBSD on x86_64.' ;;
        *:https) printf '%s\n' 'The release mirror must use HTTPS.' ;;
        *:temporary) printf '%s\n' 'Could not create a temporary download folder.' ;;
        *:failed_download) printf '%s\n' 'Download failed. Check your connection and try again.' ;;
        *:failed_metadata) printf '%s\n' 'Could not download the signed release. Nothing was installed. Check your connection and try again.' ;;
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
    name=$1 destination=$2 limit=$3 timeout=$4
    if [ "$downloader" = fetch ]; then
        fetch_base "$@"
        return
    fi
    # Keep retry warnings in logs; the terminal meter includes bytes and speed.
    if [ -t 2 ]; then progress=--progress-meter; else progress=--no-progress-meter; fi
    received=$(curl --fail --location --show-error "$progress" \
        --proto "=$protocols" --proto-redir "=$protocols" \
        --retry 3 --retry-connrefused --retry-max-time 180 \
        --connect-timeout 15 --speed-limit 1024 --speed-time 30 \
        --max-time "$timeout" --max-filesize "$limit" \
        --write-out '%{size_download}' --output "$destination" "$release_base/$name") || \
        fail "${5:-failed_download}" "$name" "$(message manual) https://github.com/theHamdiz/mandara/releases/latest"
    say "  ✓ $(message download_ok) $name ($received B)"
}

# FreeBSD's own fetch(1), when curl isn't installed: it verifies HTTPS
# certificates, retries soft failures (-a) and gives up on a transfer
# stalled for 30 seconds (-T). Everything it downloads is checked against
# the signed release before anything runs.
fetch_base() {
    if [ -t 2 ]; then quiet=; else quiet=-q; fi
    attempt=0
    until command fetch $quiet -a -T 30 -o "$destination" "$release_base/$name"; do
        attempt=$((attempt + 1))
        [ "$attempt" -le 3 ] || fail "${5:-failed_download}" "$name" "$(message manual) https://github.com/theHamdiz/mandara/releases/latest"
        sleep "$attempt"
    done
    received=$(wc -c < "$destination" | tr -d ' ')
    [ "$received" -le "$limit" ] || fail "${5:-failed_download}" "$name"
    say "  ✓ $(message download_ok) $name ($received B)"
}

# SHA-256 of a file: GNU coreutils and FreeBSD 14 both have sha256sum;
# older FreeBSD has only sha256(1).
file_sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{ print $1 }'
    else
        sha256 -q "$1"
    fi
}

# POSIX printf accepts octal escapes; no xxd, Python or shell hex extension.
# They go in the format string, not through %b: FreeBSD's printf ends a %b
# argument at its first zero byte, and keys and signatures contain them.
# The format holds nothing but escapes, so no conversion can appear in it.
hex_bytes() {
    octal=$(printf '%s\n' "$1" | LC_ALL=C awk '
        { for (i = 1; i < length($0); i += 2) {
            a = index("0123456789abcdef", substr($0, i, 1)) - 1
            b = index("0123456789abcdef", substr($0, i + 1, 1)) - 1
            printf "\\%03o", a * 16 + b
        } }')
    # shellcheck disable=SC2059
    printf "$octal"
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
    for program in tar openssl awk mktemp uname; do
        command -v "$program" >/dev/null 2>&1 || fail missing_tool "$program" "$(message manual) https://github.com/theHamdiz/mandara/releases/latest"
    done
    case $(uname -s) in
        Linux) os=linux ;;
        FreeBSD) os=freebsd ;;
        *) fail unsupported ;;
    esac
    case $(uname -m) in
        x86_64|amd64) arch=x86_64 ;;
        aarch64|arm64) arch=aarch64 ;;
        *) fail unsupported ;;
    esac
    if [ "$os" = freebsd ]; then
        # One FreeBSD build (amd64), on FreeBSD's own libc.
        [ "$arch" = x86_64 ] || fail unsupported
        libc=freebsd
        platform=freebsd
    else
        libc=gnu
        if ! { command -v getconf >/dev/null 2>&1 && getconf GNU_LIBC_VERSION >/dev/null 2>&1; }; then
            for loader in /lib/ld-musl-*.so.1 /usr/lib/ld-musl-*.so.1; do
                if [ -e "$loader" ]; then libc=musl; break; fi
            done
        fi
        platform=linux-$libc
    fi
    downloader=curl
    if ! command -v curl >/dev/null 2>&1; then
        if [ "$os" = freebsd ] && command -v fetch >/dev/null 2>&1; then
            downloader=fetch
        else
            fail missing_tool curl "$(message manual) https://github.com/theHamdiz/mandara/releases/latest"
        fi
    fi
    if ! command -v sha256sum >/dev/null 2>&1 && ! { [ "$os" = freebsd ] && command -v sha256 >/dev/null 2>&1; }; then
        fail missing_tool sha256sum "$(message manual) https://github.com/theHamdiz/mandara/releases/latest"
    fi
    protocols=https
    case "$release_base" in
        https://*) ;;
        http://127.0.0.1:*) protocols=http ;;
        *) fail https ;;
    esac
    archive="mandara-$arch-$platform.tar.gz"
    download_directory

    say "[1/5] $(message signature)"
    # Small signature files should not inherit the archive's 30-minute timeout.
    fetch SHA256SUMS "$work/SHA256SUMS" 1048576 60 failed_metadata
    fetch SHA256SUMS.sig "$work/SHA256SUMS.sig" 1024 60 failed_metadata
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
    say "  ✓ $(message signature_ok)"
    expected=$(awk -v file="$archive" '$2 == file { print $1 }' "$work/SHA256SUMS")
    case "$expected" in ''|*[!0-9a-f]*) fail missing_checksum "$archive" ;; esac
    [ "${#expected}" -eq 64 ] || fail missing_checksum "$archive"

    say "[2/5] $(message download) $arch ($libc)"
    fetch "$archive" "$work/$archive" 1073741824 1800
    say "[3/5] $(message verify)"
    actual=$(file_sha256 "$work/$archive") || fail bad_checksum
    [ "$actual" = "$expected" ] || fail bad_checksum
    say "  ✓ $(message checksum_ok)"
    say "[4/5] $(message unpack)"
    tar -xzf "$work/$archive" -C "$work" || fail failed_unpack
    installer=
    for candidate in "$work"/mandara-*/bin/mandara; do
        [ -f "$candidate" ] && [ -x "$candidate" ] || continue
        [ -z "$installer" ] || fail missing_installer
        installer=$candidate
    done
    [ -n "$installer" ] || fail missing_installer
    say ''
    say "[5/5] $(message start)"
    umask "$installer_umask"
    run_installer "$installer" install "$@"
}

main "$@"
