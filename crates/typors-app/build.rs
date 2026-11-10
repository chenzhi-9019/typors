// The loader must carry a Common-Controls v6 manifest: the runtime DLL imports
// `TaskDialogIndirect`, which only exists in comctl32 v6. Without this activation
// context `LoadLibrary` fails with "procedure not found" (127).
fn main() {
    #[cfg(windows)]
    {
        let mut resource = winresource::WindowsResource::new();
        resource.set_manifest(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls"
        version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df"
        language="*" />
    </dependentAssembly>
  </dependency>
</assembly>"#,
        );
        resource.compile().expect("embed the loader manifest");
    }
}
