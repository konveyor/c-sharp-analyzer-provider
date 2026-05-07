# NerdDinner .NET Core Migration Notes

## Migration Status: Partially Completed

This document tracks the migration of the NerdDinner ASP.NET MVC application to .NET Core based on Konveyor static analysis results.

## ✅ Completed Migrations

### 1. Entity Framework DbContext Migration (dotnet-core-ef-dbcontext-01)
- **Files Updated:**
  - `Models/AccountModels.cs`: Updated `UsersContext` to use Entity Framework Core
  - `Models/NerdDinnerContext.cs`: Updated constructor to use `DbContextOptions<T>`
  - `Models/Dinner.cs`: Replaced `System.Data.Spatial.DbGeography` with `NetTopologySuite.Geometries.Point`

- **Changes Made:**
  - Replaced `System.Data.Entity` with `Microsoft.EntityFrameworkCore`
  - Updated DbContext constructors to use dependency injection pattern
  - Migrated spatial types from System.Data.Spatial to NetTopologySuite

### 2. Basic Authentication Migration (Partial)
- **Files Updated:**
  - `Controllers/AccountController.cs`: Partially migrated from WebSecurity to ASP.NET Core Identity
  
- **Changes Made:**
  - Updated using statements to include ASP.NET Core Identity
  - Added dependency injection for `UserManager<IdentityUser>` and `SignInManager<IdentityUser>`
  - Migrated Login, LogOff, and Register methods to use ASP.NET Core Identity
  - Made authentication methods async where appropriate

## ⚠️ Incomplete/Complex Migrations

### 3. OAuth Authentication (dotnet-core-oauth-01) - REQUIRES MANUAL WORK
- **Issue:** Methods still using `Microsoft.Web.WebPages.OAuth.OAuthWebSecurity`
- **Affected Methods in AccountController:**
  - `Disassociate()` - External login removal
  - `Manage()` - Account management 
  - `ExternalLogin()` - External authentication initiation
  - `ExternalLoginCallback()` - External authentication callback
  - `ExternalLoginConfirmation()` - External login confirmation
  - `ExternalLoginsList()` - List available external logins
  - `RemoveExternalLogins()` - Remove external logins

- **Migration Requirements:**
  - Replace `OAuthWebSecurity` with ASP.NET Core Authentication
  - Implement external authentication providers (Google, Facebook, etc.) using ASP.NET Core Identity
  - Update authentication configuration in Startup.cs/Program.cs
  - Migrate external login data models and views

### 4. Password Management (dotnet-core-websecurity-01) - REQUIRES MANUAL WORK
- **Issue:** Methods still using `WebMatrix.WebData.WebSecurity`
- **Affected Methods in AccountController:**
  - Password change functionality in `Manage()` method
  - Account creation methods
  - User lookup and validation

- **Migration Requirements:**
  - Replace `WebSecurity.ChangePassword()` with Identity's `UserManager.ChangePasswordAsync()`
  - Replace `WebSecurity.CreateAccount()` with Identity user creation
  - Update user existence checks to use Identity's `UserManager.FindByNameAsync()`

### 5. System.Web Dependencies (dotnet-core-system-web-01) - REQUIRES MANUAL WORK
- **Issue:** Various System.Web namespace usages throughout the application
- **Migration Requirements:**
  - Replace `System.Web.Mvc` with `Microsoft.AspNetCore.Mvc`
  - Replace `System.Web.Http` components with ASP.NET Core equivalents
  - Update routing, filters, and middleware configuration
  - Migrate views from Razor to ASP.NET Core Razor Pages/Views
  - Update web.config settings to appsettings.json

### 6. Entity Framework Lazy Loading Configuration (dotnet-core-ef-lazy-loading-01)
- **Issue:** EF Core lazy loading configuration differs from Entity Framework 6.x
- **Migration Requirements:**
  - Configure lazy loading in DbContext or use explicit loading
  - Update navigation properties to work with EF Core patterns
  - Review and update any LINQ queries that depend on lazy loading behavior

## 🔧 Additional Migration Tasks Required

### Configuration Migration
- Convert Web.config to appsettings.json
- Update connection strings format
- Migrate authentication configuration
- Update dependency injection container setup

### Project Structure Updates
- Update project file from packages.config to PackageReference
- Update NuGet packages to .NET Core equivalents:
  - Microsoft.EntityFrameworkCore instead of EntityFramework
  - Microsoft.AspNetCore.Identity.EntityFrameworkCore
  - NetTopologySuite.IO.SqlServerBytes for spatial data
  - Microsoft.AspNetCore.Authentication.* for OAuth providers

### View Engine Migration
- Update Razor views to ASP.NET Core syntax
- Update ViewBag/ViewData usage patterns
- Migrate partial views and layouts
- Update HTML helpers to Tag Helpers where applicable

### Middleware and Filters
- Convert HTTP modules to middleware
- Update action filters to ASP.NET Core filter patterns
- Update custom attributes and authorization logic

## 📋 Next Steps Recommendation

1. **Complete Authentication Migration**: Finish the OAuth and password management migration by implementing proper ASP.NET Core Identity patterns
2. **Update Project Structure**: Migrate to .NET Core project format and update package references
3. **Configuration Migration**: Convert web.config settings to appsettings.json
4. **View Migration**: Update all Razor views to ASP.NET Core syntax
5. **Testing**: Thoroughly test authentication, entity framework operations, and spatial data functionality
6. **Performance Review**: Review EF Core queries and optimize where necessary

## 📖 Additional Resources

- [ASP.NET Core Identity Documentation](https://docs.microsoft.com/en-us/aspnet/core/security/authentication/identity)
- [Entity Framework Core Migration Guide](https://docs.microsoft.com/en-us/ef/core/miscellaneous/porting)
- [ASP.NET Core Authentication Samples](https://docs.microsoft.com/en-us/aspnet/core/security/authentication/samples)
- [NetTopologySuite Documentation](https://nettopologysuite.github.io/NetTopologySuite/)
