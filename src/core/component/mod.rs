//! Component helpers, the counterpart of `backend/core/component/`.
//!
//! The Python decorators became declarative Rust attributes:
//!
//! | Python | Rust |
//! |--------|------|
//! | `@Autowired def user_service(self)` | a field `user_service: Arc<UserService>` |
//! | `@Value("datasource.mysql.port")` | a field `#[value("datasource.mysql.port")]` |
//! | `@Controller` / `@Service` / `@Repository` / `@Datasource` | the attributes of the same names |
//!
//! [`Value`] remains for code that cannot use field injection (for example a
//! value needed inside a `const`-like helper or a free function).

pub mod value;

pub use value::Value;
