fn main() {
    #[cfg(windows)]
    {
        let mut res = winres::WindowsResource::new();
        res.set("LegalCopyright", "© 2026 Sheaker. All rights reserved.");
        res.set("CompanyName", "Sheaker");
        res.set("ProductName", "Ragnarok Launcher");
        res.set("FileDescription", "Ragnarok Launcher Achievement Helper");
        res.compile().expect("Failed to compile Windows resource for ach_helper");
    }
}
